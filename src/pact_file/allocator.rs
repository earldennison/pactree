use std::collections::{BTreeMap, BTreeSet};

use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub(crate) struct Extent {
    pub(crate) offset: u64,
    pub(crate) len: u64,
}

impl Extent {
    fn end(self) -> PactResult<u64> {
        self.offset
            .checked_add(self.len)
            .ok_or_else(|| PactError::invalid("Pact extent end overflow"))
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct ExtentAllocator {
    pub(crate) file_end: u64,
    pub(crate) live: BTreeSet<Extent>,
    pub(crate) free: BTreeSet<Extent>,
    pub(crate) retired: BTreeMap<u64, Vec<Extent>>,
}

impl ExtentAllocator {
    pub(crate) fn new(file_end: u64) -> Self {
        Self {
            file_end,
            ..Self::default()
        }
    }

    pub(crate) fn from_parts(
        file_end: u64,
        live: Vec<Extent>,
        free: Vec<Extent>,
        retired: Vec<(u64, Extent)>,
    ) -> PactResult<Self> {
        let live = live.into_iter().collect::<BTreeSet<_>>();
        let free = free.into_iter().collect::<BTreeSet<_>>();
        let mut retired_by_generation = BTreeMap::<u64, Vec<Extent>>::new();
        for (generation, extent) in retired {
            if generation == 0 {
                return Err(PactError::invalid(
                    "Pact retired extent requires a positive generation",
                ));
            }
            retired_by_generation
                .entry(generation)
                .or_default()
                .push(extent);
        }
        let mut allocator = Self {
            file_end,
            live,
            free,
            retired: retired_by_generation,
        };
        allocator.validate()?;
        allocator.coalesce_free()?;
        Ok(allocator)
    }

    pub(crate) fn allocate(&mut self, len: u64, _generation: u64) -> PactResult<Extent> {
        if len == 0 {
            return Err(PactError::invalid("Pact extent allocation cannot be empty"));
        }
        let selected = self
            .free
            .iter()
            .copied()
            .filter(|extent| extent.len >= len)
            .min_by_key(|extent| (extent.len, extent.offset));
        let extent = if let Some(selected) = selected {
            self.free.remove(&selected);
            let allocated = Extent {
                offset: selected.offset,
                len,
            };
            if selected.len > len {
                self.free.insert(Extent {
                    offset: selected
                        .offset
                        .checked_add(len)
                        .ok_or_else(|| PactError::invalid("Pact extent split overflow"))?,
                    len: selected.len - len,
                });
            }
            allocated
        } else {
            let allocated = Extent {
                offset: self.file_end,
                len,
            };
            self.file_end = allocated.end()?;
            allocated
        };
        if !self.live.insert(extent) {
            return Err(PactError::internal(
                "Pact allocator produced an already-live extent",
            ));
        }
        Ok(extent)
    }

    pub(crate) fn retire(&mut self, extent: Extent, release_generation: u64) -> PactResult<()> {
        if release_generation == 0 {
            return Err(PactError::invalid(
                "Pact retired extent requires a positive generation",
            ));
        }
        if !self.live.remove(&extent) {
            return Err(PactError::invalid(
                "Pact allocator cannot retire an extent that is not live",
            ));
        }
        self.retired
            .entry(release_generation)
            .or_default()
            .push(extent);
        Ok(())
    }

    pub(crate) fn release_before_generation(&mut self, generation: u64) -> PactResult<()> {
        let releasable = self
            .retired
            .range(..generation)
            .map(|(generation, _)| *generation)
            .collect::<Vec<_>>();
        for retired_generation in releasable {
            if let Some(extents) = self.retired.remove(&retired_generation) {
                self.free.extend(extents);
            }
        }
        self.coalesce_free()?;
        self.validate()
    }

    fn coalesce_free(&mut self) -> PactResult<()> {
        let mut merged = Vec::<Extent>::new();
        for extent in self.free.iter().copied() {
            if extent.len == 0 {
                return Err(PactError::invalid("Pact free extent cannot be empty"));
            }
            if let Some(previous) = merged.last_mut() {
                let previous_end = previous.end()?;
                if previous_end > extent.offset {
                    return Err(PactError::invalid("Pact free extents overlap"));
                }
                if previous_end == extent.offset {
                    previous.len = previous
                        .len
                        .checked_add(extent.len)
                        .ok_or_else(|| PactError::invalid("Pact free extent merge overflow"))?;
                    continue;
                }
            }
            merged.push(extent);
        }
        self.free = merged.into_iter().collect();
        Ok(())
    }

    fn validate(&self) -> PactResult<()> {
        let mut extents = self
            .live
            .iter()
            .chain(self.free.iter())
            .copied()
            .chain(self.retired.values().flatten().copied())
            .collect::<Vec<_>>();
        extents.sort_by_key(|extent| (extent.offset, extent.len));
        for extent in &extents {
            if extent.len == 0 || extent.end()? > self.file_end {
                return Err(PactError::invalid(
                    "Pact allocator extent exceeds the file boundary",
                ));
            }
        }
        for pair in extents.windows(2) {
            if pair[0].end()? > pair[1].offset {
                return Err(PactError::invalid("Pact allocator extents overlap"));
            }
        }
        Ok(())
    }
}
