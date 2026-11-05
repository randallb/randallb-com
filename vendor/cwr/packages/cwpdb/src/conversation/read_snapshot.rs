//! Public finite selections over a frozen native view or a revision-pinned remote view.
use super::*;
use serde::de::DeserializeOwned;
use std::sync::{Mutex, TryLockError};

/// A trusted host bridges its current backend and authorizer. This port accepts
/// only the declared operation enum; no SQL, handles or query callback crosses it.
pub trait ReadSelectionSource: Send {
    fn read_selection(
        &mut self,
        request: &ReadSelectionRequest,
        control: &ReadControl,
    ) -> CwpdbResult<ReadSelectionFacts>;
}
pub enum ReadSnapshot {
    Native(NativeReadSnapshot),
    Remote(RemoteReadSnapshot),
}
impl From<NativeReadSnapshot> for ReadSnapshot {
    fn from(snapshot: NativeReadSnapshot) -> Self {
        Self::Native(snapshot)
    }
}
pub struct RemoteReadSnapshot {
    source: Mutex<Box<dyn ReadSelectionSource>>,
    scope: Scope,
    control: ReadControl,
    commit_revision: String,
}
impl ReadSnapshot {
    pub fn remote(
        scope: Scope,
        control: ReadControl,
        source: Box<dyn ReadSelectionSource>,
    ) -> CwpdbResult<Self> {
        let mut snapshot = RemoteReadSnapshot {
            source: Mutex::new(source),
            scope,
            control,
            commit_revision: String::new(),
        };
        let facts = snapshot.read_facts(ReadSelectionOperation::Position, None)?;
        if !facts.result.is_null() {
            return Err("invalid read selection position facts".into());
        }
        snapshot.commit_revision = facts.commit_revision;
        Ok(Self::Remote(snapshot))
    }
    pub fn runtime_root_ref(&self) -> &str {
        match self {
            Self::Native(snapshot) => snapshot.runtime_root_ref(),
            Self::Remote(snapshot) => &snapshot.scope.runtime_root_ref,
        }
    }
    pub fn commit_revision(&self) -> &str {
        match self {
            Self::Native(snapshot) => snapshot.commit_revision(),
            Self::Remote(snapshot) => &snapshot.commit_revision,
        }
    }
    #[cfg(feature = "test-support")]
    pub fn cancellation_probe(&self, exhaustive: bool) -> CwpdbResult<i64> {
        match self {
            Self::Native(snapshot) => snapshot.cancellation_probe(exhaustive),
            Self::Remote(_) => {
                Err("native cancellation probe is unavailable for remote selections".into())
            }
        }
    }
}
impl RemoteReadSnapshot {
    fn read_facts(
        &self,
        selection: ReadSelectionOperation,
        expected: Option<String>,
    ) -> CwpdbResult<ReadSelectionFacts> {
        self.control.check()?;
        let remaining = self
            .control
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .checked_add(remaining)
            .ok_or("read deadline overflow")?;
        let request = ReadSelectionRequest {
            scope: self.scope.clone(),
            expected_commit_revision: expected.clone(),
            selection,
            deadline_unix_ms: i64::try_from(deadline.as_millis())?,
            max_bytes: MAX_WIRE_BYTES,
        };
        let mut source = loop {
            self.control.check()?;
            match self.source.try_lock() {
                Ok(source) => break source,
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err("read selection source lock poisoned".into())
                }
            }
        };
        let facts = source.read_selection(&request, &self.control)?;
        self.control.check()?;
        if facts.runtime_root_ref != self.scope.runtime_root_ref
            || facts
                .commit_revision
                .parse::<i64>()
                .ok()
                .is_none_or(|revision| revision < 0)
            || expected
                .as_ref()
                .is_some_and(|revision| revision != &facts.commit_revision)
        {
            return Err("conversation read snapshot revision conflict".into());
        }
        if serde_json::to_vec(&facts)?.len() > MAX_WIRE_BYTES {
            return Err("read selection exceeds response byte budget".into());
        }
        Ok(facts)
    }
    fn read<T: DeserializeOwned>(&self, selection: ReadSelectionOperation) -> CwpdbResult<T> {
        Ok(serde_json::from_value(
            self.read_facts(selection, Some(self.commit_revision.clone()))?
                .result,
        )?)
    }
}
mod current;
mod inspector;
mod metadata;
mod publication;
mod summaries;
mod timeline;

mod pages;
