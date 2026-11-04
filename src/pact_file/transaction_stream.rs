use std::collections::VecDeque;

use super::index::IndexKey;
use super::snapshot::Snapshot;
use super::transaction::PactTransaction;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

pub(crate) const GENERATION_PAGE_ENTRIES: usize = 256;

pub(crate) struct TransactionStream<'a> {
    snapshot: &'a Snapshot,
    pending: VecDeque<u64>,

    next_generation: Option<u64>,
    failed: bool,

    derived_branches: Option<std::collections::BTreeSet<String>>,
}

impl<'a> TransactionStream<'a> {
    pub(crate) fn new(snapshot: &'a Snapshot) -> Self {
        Self {
            snapshot,
            pending: VecDeque::new(),
            next_generation: Some(0),
            failed: false,
            derived_branches: None,
        }
    }

    pub(crate) fn skipping_derived_depth(
        mut self,
        branches: std::collections::BTreeSet<String>,
    ) -> Self {
        self.derived_branches = Some(branches);
        self
    }

    fn fetch_page(&mut self) -> PactResult<()> {
        let Some(start) = self.next_generation else {
            return Ok(());
        };
        let entries = self.snapshot.index_range(
            &IndexKey::Generation { generation: start },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            GENERATION_PAGE_ENTRIES,
            false,
        )?;
        let mut last_generation = None;
        for (key, value) in entries {
            let IndexKey::Generation { generation } = IndexKey::decode(&key)? else {
                return Err(PactError::corrupt(
                    "Pact generation range returned another namespace",
                ));
            };
            let raw: [u8; 8] = value
                .as_slice()
                .try_into()
                .map_err(|_| PactError::corrupt("Pact transaction address has invalid width"))?;
            self.pending.push_back(u64::from_le_bytes(raw));
            last_generation = Some(generation);
        }
        self.next_generation = match last_generation {
            Some(generation) if self.pending.len() == GENERATION_PAGE_ENTRIES => {
                generation.checked_add(1)
            }

            _ => None,
        };
        Ok(())
    }
}

impl Iterator for TransactionStream<'_> {
    type Item = PactResult<(PactTransaction, u64)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        while self.pending.is_empty() {
            self.next_generation?;
            if let Err(error) = self.fetch_page() {
                self.failed = true;
                return Some(Err(error));
            }
        }
        let address = self.pending.pop_front()?;
        match self
            .snapshot
            .decode_transaction_at_with(address, self.derived_branches.as_ref())
        {
            Ok(item) => Some(Ok(item)),
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}
