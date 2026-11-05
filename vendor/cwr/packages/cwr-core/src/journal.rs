//! In-memory undo records for the retained-state SQLite transaction. Reads do
//! not copy history; the first write to a record saves just that record.
use serde::{Deserialize, Serialize};
use std::{
    borrow::Borrow,
    collections::BTreeMap,
    ops::{Deref, Index, IndexMut},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent, bound(deserialize = "T: Deserialize<'de>"))]
pub(crate) struct JournalVec<T> {
    values: Vec<T>,
    #[serde(skip)]
    originals: BTreeMap<usize, T>,
    #[serde(skip)]
    original_len: Option<usize>,
}

impl<T> Default for JournalVec<T> {
    fn default() -> Self {
        Vec::new().into()
    }
}
impl<T> JournalVec<T> {
    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}
impl<T> From<Vec<T>> for JournalVec<T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values,
            originals: BTreeMap::new(),
            original_len: None,
        }
    }
}
impl<T> Deref for JournalVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.values
    }
}
impl<T> Index<usize> for JournalVec<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.values[index]
    }
}
impl<T: Clone> IndexMut<usize> for JournalVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).unwrap()
    }
}
impl<T: Clone> JournalVec<T> {
    /// Test snapshots use the normal change journal; retained ordered lists are
    /// append-only. Named corruption fixtures own destructive physical edits.
    #[cfg(test)]
    pub(crate) fn fixture_replace(&mut self, replacement: &Self) -> crate::CwrResult<()> {
        if replacement.len() < self.len() {
            return Err("fixture replacement cannot truncate an append-only retained list".into());
        }
        for (index, value) in replacement.iter().enumerate() {
            if index < self.len() {
                *self.get_mut(index).unwrap() = value.clone();
            } else {
                self.push(value.clone());
            }
        }
        Ok(())
    }
    pub(crate) fn begin(&mut self) {
        assert!(self.original_len.is_none());
        self.original_len = Some(self.values.len());
    }
    pub(crate) fn commit(&mut self) {
        self.original_len = None;
        self.originals.clear();
    }
    pub(crate) fn rollback(&mut self) {
        if let Some(len) = self.original_len.take() {
            self.values.truncate(len);
        }
        for (index, value) in std::mem::take(&mut self.originals) {
            self.values[index] = value;
        }
    }
    pub(crate) fn changes(&self) -> impl Iterator<Item = (usize, Option<&T>, &T)> {
        self.originals
            .iter()
            .map(|(&index, old)| (index, Some(old), &self.values[index]))
            .chain(
                (self.original_len.unwrap_or(self.values.len())..self.values.len())
                    .map(|index| (index, None, &self.values[index])),
            )
    }
    pub(crate) fn push(&mut self, value: T) {
        self.values.push(value);
    }
    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        let value = self.values.get_mut(index)?;
        if self.original_len.is_some_and(|len| index < len) {
            self.originals.entry(index).or_insert_with(|| value.clone());
        }
        Some(value)
    }
    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        self.get_mut(self.values.len().checked_sub(1)?)
    }
    #[cfg(test)]
    pub(crate) fn first_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }
    pub(crate) fn find_mut(&mut self, predicate: impl FnMut(&T) -> bool) -> Option<&mut T> {
        let index = self.values.iter().position(predicate)?;
        self.get_mut(index)
    }
    pub(crate) fn matching_mut(
        &mut self,
        mut predicate: impl FnMut(&T) -> bool,
    ) -> impl Iterator<Item = &mut T> {
        let len = self.original_len;
        let originals = &mut self.originals;
        self.values
            .iter_mut()
            .enumerate()
            .filter_map(move |(index, value)| {
                if !predicate(value) {
                    return None;
                }
                if len.is_some_and(|len| index < len) {
                    originals.entry(index).or_insert_with(|| value.clone());
                }
                Some(value)
            })
    }
    #[cfg(test)]
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.matching_mut(|_| true)
    }
}
impl<'a, T> IntoIterator for &'a JournalVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}
impl<'a, T: Clone> IntoIterator for &'a mut JournalVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        for index in 0..self.original_len.unwrap_or(0) {
            self.originals
                .entry(index)
                .or_insert_with(|| self.values[index].clone());
        }
        self.values.iter_mut()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    transparent,
    bound(deserialize = "K: Ord + Deserialize<'de>, V: Deserialize<'de>")
)]
pub(crate) struct JournalMap<K: Ord, V> {
    values: BTreeMap<K, V>,
    #[serde(skip)]
    originals: BTreeMap<K, Option<V>>,
    #[serde(skip)]
    tracking: bool,
}
impl<K: Ord, V> Default for JournalMap<K, V> {
    fn default() -> Self {
        BTreeMap::new().into()
    }
}
impl<K: Ord, V> From<BTreeMap<K, V>> for JournalMap<K, V> {
    fn from(values: BTreeMap<K, V>) -> Self {
        Self {
            values,
            originals: BTreeMap::new(),
            tracking: false,
        }
    }
}
impl<K: Ord, V> Deref for JournalMap<K, V> {
    type Target = BTreeMap<K, V>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl<K: Ord + Clone, V: Clone> JournalMap<K, V> {
    #[cfg(test)]
    pub(crate) fn fixture_replace(&mut self, replacement: &Self) {
        let removed = self
            .keys()
            .filter(|key| !replacement.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed {
            self.remove(&key);
        }
        for (key, value) in replacement.iter() {
            self.insert(key.clone(), value.clone());
        }
    }
    pub(crate) fn begin(&mut self) {
        assert!(!self.tracking);
        self.tracking = true;
    }
    pub(crate) fn commit(&mut self) {
        self.tracking = false;
        self.originals.clear();
    }
    pub(crate) fn rollback(&mut self) {
        for (key, value) in std::mem::take(&mut self.originals) {
            match value {
                Some(value) => {
                    self.values.insert(key, value);
                }
                None => {
                    self.values.remove(&key);
                }
            }
        }
        self.tracking = false;
    }
    pub(crate) fn changes(&self) -> impl Iterator<Item = (&K, Option<&V>, Option<&V>)> {
        self.originals
            .iter()
            .map(|(key, old)| (key, old.as_ref(), self.values.get(key)))
    }
    fn remember(&mut self, key: &K) {
        if self.tracking {
            self.originals
                .entry(key.clone())
                .or_insert_with(|| self.values.get(key).cloned());
        }
    }
    pub(crate) fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let owned = self.values.get_key_value(key)?.0.clone();
        self.remember(&owned);
        self.values.get_mut(key)
    }
    pub(crate) fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let owned = self.values.get_key_value(key)?.0.clone();
        self.remember(&owned);
        self.values.remove(key)
    }
    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.remember(&key);
        self.values.insert(key, value)
    }
}
impl<'a, K: Ord, V> IntoIterator for &'a JournalMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::collections::btree_map::Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}
