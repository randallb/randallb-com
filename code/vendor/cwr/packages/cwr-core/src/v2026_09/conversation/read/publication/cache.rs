//! Selection-owned memoization. Callers serialize access through the snapshot.
use super::*;
use std::{cell::RefCell, collections::BTreeMap};

pub(super) struct ReadCache<V>(RefCell<BTreeMap<String, V>>);
impl<V> Default for ReadCache<V> {
    fn default() -> Self {
        Self(RefCell::new(BTreeMap::new()))
    }
}
impl<V: Clone> ReadCache<V> {
    #[cfg(test)]
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.0.borrow().contains_key(key)
    }
    pub(super) fn get_or_load(
        &self,
        key: &str,
        load: impl FnOnce() -> CwrResult<V>,
    ) -> CwrResult<V> {
        if let Some(value) = self.0.borrow().get(key) {
            return Ok(value.clone());
        }
        // Do not hold a RefCell borrow while another cache may be consulted.
        // Operational errors are never cached as missing/unauthorized resources.
        let value = load()?;
        self.0.borrow_mut().insert(key.into(), value.clone());
        Ok(value)
    }
}
#[derive(Default)]
pub(super) struct PublicationReadCache {
    pub resources: ReadCache<Option<std::sync::Arc<ConversationResourceSnapshot>>>,
    pub conversations: ReadCache<bool>,
    pub actors: ReadCache<ConversationActorPresentation>,
    pub metadata: ReadCache<Option<ConversationMessageMetadata>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_cache_loads_values_and_absence_once_but_retries_errors() {
        let cache = ReadCache::<Option<u32>>::default();
        let calls = std::cell::Cell::new(0);
        for _ in 0..3 {
            assert_eq!(
                cache
                    .get_or_load("missing", || {
                        calls.set(calls.get() + 1);
                        Ok(None)
                    })
                    .unwrap(),
                None
            );
            assert_eq!(
                cache
                    .get_or_load("found", || {
                        calls.set(calls.get() + 1);
                        Ok(Some(42))
                    })
                    .unwrap(),
                Some(42)
            );
        }
        assert_eq!(calls.get(), 2);
        assert!(cache
            .get_or_load("retry", || Err(std::io::Error::other("cancelled").into()))
            .is_err());
        assert_eq!(cache.get_or_load("retry", || Ok(Some(1))).unwrap(), Some(1));
        assert_eq!(
            ReadCache::default()
                .get_or_load("found", || Ok(Some(7)))
                .unwrap(),
            Some(7)
        );
    }
}
