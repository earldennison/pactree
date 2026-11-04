use crate::error::{PactError, PactResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PactMemoryOptions {
    pub page_cache_bytes: usize,

    pub component_cache_bytes: usize,

    pub scratch_cache_bytes: usize,

    pub io_chunk_bytes: usize,

    pub in_flight_bytes: usize,
}

impl Default for PactMemoryOptions {
    fn default() -> Self {
        Self {
            page_cache_bytes: 8 * 1024 * 1024,
            component_cache_bytes: 8 * 1024 * 1024,
            scratch_cache_bytes: 8 * 1024 * 1024,
            io_chunk_bytes: 256 * 1024,
            in_flight_bytes: 8 * 1024 * 1024,
        }
    }
}

impl PactMemoryOptions {
    pub fn validate(&self) -> PactResult<()> {
        if self.io_chunk_bytes == 0 {
            return Err(PactError::invalid(
                "Pact memory options require a positive io_chunk_bytes",
            ));
        }
        self.page_cache_bytes
            .checked_add(self.component_cache_bytes)
            .and_then(|total| total.checked_add(self.scratch_cache_bytes))
            .and_then(|total| total.checked_add(self.io_chunk_bytes))
            .and_then(|total| total.checked_add(self.in_flight_bytes))
            .ok_or_else(|| PactError::invalid("Pact memory options overflow usize"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PactStorageMemory {
    pub cache_resident_bytes: usize,

    pub in_flight_bytes: usize,

    pub in_flight_peak_bytes: usize,
}

pub fn storage_memory() -> PactStorageMemory {
    PactStorageMemory {
        cache_resident_bytes: crate::pact_file::memory_accounting::cache_resident_bytes(),
        in_flight_bytes: crate::pact_file::memory_accounting::in_flight_bytes(),
        in_flight_peak_bytes: crate::pact_file::memory_accounting::in_flight_peak_bytes(),
    }
}

pub fn reset_storage_memory_peak() {
    crate::pact_file::memory_accounting::reset_in_flight_peak();
}
