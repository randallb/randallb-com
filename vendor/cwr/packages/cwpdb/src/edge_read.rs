//! Direct relationship reads inside an already-authorized domain scope.

use crate::CwpdbResult;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Incoming,
    Outgoing,
}

/// Selected relationship contracts registered by the owning domain. The domain
/// must not label an ordered kind unordered to use this first query interface.
#[derive(Debug, Clone, Copy)]
pub enum RelationshipKind<'a> {
    Unordered(&'a str),
    Ordered(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeRevision(i64);

/// Complete relationship data without database-local identities or positions.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRecord {
    pub reference: String,
    pub source_ref: String,
    pub target_ref: String,
    pub kind: String,
    pub schema_ref: String,
    pub properties: Value,
    pub revision: EdgeRevision,
}

#[derive(Debug)]
pub enum EdgeReadError {
    InvalidInput,
    UnsupportedDomainContract,
    InvalidContinuation,
    Storage(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for EdgeReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => {
                f.write_str("edge query needs nonempty kinds and a positive page size")
            }
            Self::UnsupportedDomainContract => {
                f.write_str("ordered relationships require a different query contract")
            }
            Self::InvalidContinuation => f.write_str("invalid edge query continuation"),
            Self::Storage(error) => write!(f, "edge query failed: {error}"),
        }
    }
}

impl std::error::Error for EdgeReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryIdentity {
    reference: String,
    direction: Direction,
    kinds: BTreeSet<String>,
    page_size: usize,
}

/// In-process cursor, usable only with its original reader and query. It is not
/// a wire token and does not survive reopening a reader, connection, or runtime.
#[derive(Clone)]
pub struct EdgeContinuation {
    scope: Arc<()>,
    query: QueryIdentity,
    after: String,
}

impl std::fmt::Debug for EdgeContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeContinuation").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct EdgePage {
    pub items: Vec<EdgeRecord>,
    pub continuation: Option<EdgeContinuation>,
    pub complete: bool,
}

pub struct EdgeQuery<'a> {
    pub direction: Direction,
    pub kinds: &'a [RelationshipKind<'a>],
    pub page_size: usize,
    pub continuation: Option<&'a EdgeContinuation>,
}

/// A non-transferable query scope for one connection and authorized caller.
/// Like get_ref, this is an internal storage primitive, not authorization or a
/// public CWR endpoint. The domain registers the selected kinds and authorizes
/// the starting ref, edges, and endpoints before using this reader. Reauthorize
/// on every page; discard the reader when caller authority changes. Do not share
/// it between callers. Continuations cannot grant access or preserve authority.
///
/// Each page reads one SQLite snapshot. Pages do not promise a common snapshot:
/// inserts before the cursor can be missed, and removed edges can disappear.
/// Use a caller-owned read transaction when a domain requires one stable report.
pub struct EdgeReader<'db> {
    db: &'db Connection,
    scope: Arc<()>,
}

impl<'db> EdgeReader<'db> {
    pub fn new(db: &'db Connection) -> Self {
        Self {
            db,
            scope: Arc::new(()),
        }
    }

    pub fn query_edges(
        &self,
        reference: &str,
        options: EdgeQuery<'_>,
    ) -> Result<EdgePage, EdgeReadError> {
        let mut kinds = BTreeSet::new();
        for kind in options.kinds {
            match kind {
                RelationshipKind::Ordered(_) => {
                    return Err(EdgeReadError::UnsupportedDomainContract)
                }
                RelationshipKind::Unordered(name) if !name.is_empty() => {
                    kinds.insert((*name).to_owned());
                }
                _ => return Err(EdgeReadError::InvalidInput),
            }
        }
        let limit = options
            .page_size
            .checked_add(1)
            .and_then(|n| i64::try_from(n).ok())
            .filter(|_| options.page_size > 0 && !kinds.is_empty())
            .ok_or(EdgeReadError::InvalidInput)?;
        let query = QueryIdentity {
            reference: reference.to_owned(),
            direction: options.direction,
            kinds,
            page_size: options.page_size,
        };
        let after = match options.continuation {
            Some(token) if Arc::ptr_eq(&self.scope, &token.scope) && token.query == query => {
                &token.after
            }
            Some(_) => return Err(EdgeReadError::InvalidContinuation),
            None => "",
        };
        let read = || -> CwpdbResult<Vec<EdgeRecord>> {
            let endpoint = match options.direction {
                Direction::Incoming => "target_id",
                Direction::Outgoing => "source_id",
            };
            // Only the enum-selected column enters SQL. Caller data stays bound.
            let sql = format!(
                "SELECT r.canonical_ref,src.canonical_ref,dst.canonical_ref,e.kind,
                        s.canonical_ref,json(e.props),e.revision
                 FROM edges e JOIN ref_bindings r ON r.ref_id=e.ref_id
                 JOIN ref_bindings src ON src.ref_id=e.source_id
                 JOIN ref_bindings dst ON dst.ref_id=e.target_id
                 JOIN ref_bindings s ON s.ref_id=e.schema_id
                 WHERE e.{endpoint}=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
                   AND e.kind IN (SELECT value FROM json_each(?2))
                   AND r.canonical_ref>?3
                 ORDER BY r.canonical_ref LIMIT ?4"
            );
            let mut statement = self.db.prepare_cached(&sql)?;
            let rows = statement.query_map(
                params![
                    reference,
                    serde_json::to_string(&query.kinds)?,
                    after,
                    limit
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get::<_, String>(5)?,
                        row.get(6)?,
                    ))
                },
            )?;
            rows.map(|row| {
                let (reference, source_ref, target_ref, kind, schema_ref, properties, revision) =
                    row?;
                Ok(EdgeRecord {
                    reference,
                    source_ref,
                    target_ref,
                    kind,
                    schema_ref,
                    properties: serde_json::from_str(&properties)?,
                    revision: EdgeRevision(revision),
                })
            })
            .collect()
        };
        let mut items = read().map_err(EdgeReadError::Storage)?;
        let complete = items.len() <= options.page_size;
        items.truncate(options.page_size);
        let continuation = if complete {
            None
        } else {
            Some(EdgeContinuation {
                scope: self.scope.clone(),
                query,
                after: items.last().expect("positive page size").reference.clone(),
            })
        };
        Ok(EdgePage {
            items,
            continuation,
            complete,
        })
    }
}

#[cfg(test)]
#[path = "edge_read_tests.rs"]
mod tests;
