//! Small retained metadata collections share cursor validation and Relay slicing.
use super::{publications::Cursor, schema::PageInfo};
use async_graphql::Result;
pub(super) struct Window {
    pub(super) first: Option<i32>,
    pub(super) after: Option<Cursor>,
    pub(super) last: Option<i32>,
    pub(super) before: Option<Cursor>,
}
pub(super) struct Page<T> {
    pub(super) values: Vec<(Cursor, T)>,
    pub(super) info: PageInfo,
    pub(super) total: i32,
}
impl Window {
    pub(super) fn select<T>(self, scope: &str, values: Vec<(usize, String, T)>) -> Result<Page<T>> {
        let size = self.first.or(self.last).unwrap_or(50);
        if !(1..=100).contains(&size) || self.first.is_some() && self.last.is_some() {
            return Err("Select first or last between 1 and 100".into());
        }
        let boundary = |cursor: Option<Cursor>| -> Result<Option<usize>> {
            cursor
                .map(|c| {
                    let (actual, index, key): (String, usize, String) = serde_json::from_str(&c.0)?;
                    if actual != scope {
                        return Err("Invalid cursor scope".into());
                    }
                    if !values.iter().any(|(i, k, _)| *i == index && *k == key) {
                        return Err("Cursor entry unavailable; request a fresh page".into());
                    }
                    Ok(index)
                })
                .transpose()
        };
        let after = boundary(self.after)?;
        let before = boundary(self.before)?;
        if after.zip(before).is_some_and(|(a, b)| a >= b) {
            return Err("Invalid cursor range".into());
        }
        let total = i32::try_from(values.len())?;
        let mut values = values
            .into_iter()
            .filter(|(i, _, _)| after.is_none_or(|a| *i > a) && before.is_none_or(|b| *i < b))
            .collect::<Vec<_>>();
        let truncated = values.len() > size as usize;
        if self.last.is_some() {
            values.drain(..values.len().saturating_sub(size as usize));
        } else {
            values.truncate(size as usize);
        }
        let values = values
            .into_iter()
            .map(|(index, key, value)| {
                Ok((Cursor(serde_json::to_string(&(scope, index, key))?), value))
            })
            .collect::<Result<Vec<_>>>()?;
        let info = PageInfo::new(
            self.last.is_none() && truncated,
            self.last.is_some() && truncated,
            values.first().map(|(c, _)| c.0.clone()),
            values.last().map(|(c, _)| c.0.clone()),
        );
        Ok(Page {
            values,
            info,
            total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_count_before_slicing_and_reject_changed_entries_or_scope() {
        let values = || {
            vec![
                (0, "a".into(), 10),
                (2, "b".into(), 20),
                (3, "c".into(), 30),
            ]
        };
        let page = Window {
            first: Some(1),
            after: None,
            last: None,
            before: None,
        }
        .select("scope", values())
        .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.values[0].1, 10);
        let cursor = page.values[0].0.clone();
        let page = Window {
            first: None,
            after: Some(cursor.clone()),
            last: Some(1),
            before: None,
        }
        .select("scope", values())
        .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.values[0].1, 30);
        assert!(Window {
            first: Some(1),
            after: Some(cursor.clone()),
            last: None,
            before: None
        }
        .select("other", values())
        .is_err());
        let changed = vec![(0, "replacement".into(), 10), (2, "b".into(), 20)];
        assert!(Window {
            first: Some(1),
            after: Some(cursor),
            last: None,
            before: None
        }
        .select("scope", changed)
        .is_err());
    }
}
