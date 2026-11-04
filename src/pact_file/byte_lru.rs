use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use super::memory_accounting;

#[cfg(test)]
mod tests;

pub(crate) const ENTRY_OVERHEAD_BYTES: usize = 96;

pub(crate) struct ByteLru<K, V> {
    budget: usize,
    resident: usize,
    tick: u64,
    entries: HashMap<K, Entry<V>>,
    recency: BTreeMap<u64, K>,
}

struct Entry<V> {
    tick: u64,
    charge: usize,
    value: V,
}

impl<K: Hash + Eq + Clone, V> ByteLru<K, V> {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            budget,
            resident: 0,
            tick: 0,
            entries: HashMap::new(),
            recency: BTreeMap::new(),
        }
    }

    pub(crate) fn budget(&self) -> usize {
        self.budget
    }

    pub(crate) fn resident_bytes(&self) -> usize {
        self.resident
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn charge_for(value_bytes: usize) -> usize {
        value_bytes
            .saturating_add(std::mem::size_of::<K>())
            .saturating_add(ENTRY_OVERHEAD_BYTES)
    }

    pub(crate) fn fits(&self, value_bytes: usize) -> bool {
        Self::charge_for(value_bytes) <= self.budget
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<&V> {
        let entry = self.entries.get_mut(key)?;
        self.recency.remove(&entry.tick);
        self.tick += 1;
        entry.tick = self.tick;
        self.recency.insert(self.tick, key.clone());
        Some(&entry.value)
    }

    pub(crate) fn insert(&mut self, key: K, value: V, value_bytes: usize) -> Result<(), V> {
        let charge = Self::charge_for(value_bytes);
        if charge > self.budget {
            return Err(value);
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.recency.remove(&previous.tick);
            self.release(previous.charge);
        }
        while self.resident.saturating_add(charge) > self.budget {
            let Some((_, oldest_key)) = self.recency.pop_first() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest_key) {
                self.release(evicted.charge);
            }
        }
        self.tick += 1;
        self.resident = self.resident.saturating_add(charge);
        memory_accounting::cache_resident_add(charge);
        self.entries.insert(
            key.clone(),
            Entry {
                tick: self.tick,
                charge,
                value,
            },
        );
        self.recency.insert(self.tick, key);
        Ok(())
    }

    fn release(&mut self, charge: usize) {
        self.resident = self.resident.saturating_sub(charge);
        memory_accounting::cache_resident_sub(charge);
    }
}

impl<K, V> Drop for ByteLru<K, V> {
    fn drop(&mut self) {
        memory_accounting::cache_resident_sub(self.resident);
    }
}
