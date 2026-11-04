use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::store::ContextTreeLedger;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LedgerCacheKey {
    pub(crate) path: PathBuf,
    pub(crate) platform_id: (u64, u128),
    pub(crate) file_identity: String,
    pub(crate) generation: u64,
    pub(crate) committed_len: u64,
}

impl LedgerCacheKey {
    pub(crate) fn new(
        canonical_path: &Path,
        platform_id: (u64, u128),
        file_identity: String,
        generation: u64,
        committed_len: u64,
    ) -> Self {
        Self {
            path: canonical_path.to_path_buf(),
            platform_id,
            file_identity,
            generation,
            committed_len,
        }
    }
}

type CacheEntry = Option<(LedgerCacheKey, Weak<ContextTreeLedger>)>;

static ENTRY: OnceLock<Mutex<CacheEntry>> = OnceLock::new();

fn entry() -> &'static Mutex<CacheEntry> {
    ENTRY.get_or_init(|| Mutex::new(None))
}

pub(crate) fn get(key: &LedgerCacheKey) -> Option<Arc<ContextTreeLedger>> {
    let mut slot = entry().lock().ok()?;
    match slot.as_ref() {
        Some((cached, weak)) if cached == key => match weak.upgrade() {
            Some(ledger) => Some(ledger),
            None => {
                *slot = None;
                None
            }
        },
        _ => None,
    }
}

pub(crate) fn put(key: LedgerCacheKey, ledger: &Arc<ContextTreeLedger>) {
    if let Ok(mut slot) = entry().lock() {
        *slot = Some((key, Arc::downgrade(ledger)));
    }
}
