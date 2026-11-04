use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PactArrayError {
    InvalidDepth { depth: i32 },
    ProtectedDepthRemoval { depth: i32 },
    SparseDepthInsert { depth: i32, max_depth: i32 },
    MissingDepth { depth: i32 },
    MissingOffset { offset: i32 },
    MissingCore,
    InvalidReplacement { coordinate: String },
}

impl fmt::Display for PactArrayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDepth { depth } => write!(formatter, "Invalid PACT depth: {depth}"),
            Self::ProtectedDepthRemoval { depth } => {
                write!(formatter, "Cannot remove protected PACT depth {depth}")
            }
            Self::SparseDepthInsert { depth, max_depth } => write!(
                formatter,
                "Cannot insert sparse PACT depth {depth}; current max depth is {max_depth}"
            ),
            Self::MissingDepth { depth } => write!(formatter, "No component at depth {depth}"),
            Self::MissingOffset { offset } => write!(formatter, "No item at offset {offset}"),
            Self::MissingCore => write!(formatter, "No item at offset 0"),
            Self::InvalidReplacement { coordinate } => {
                write!(
                    formatter,
                    "Cannot replace missing PACT coordinate {coordinate}"
                )
            }
        }
    }
}

impl std::error::Error for PactArrayError {}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct OffsetShift {
    pub from_offset: i32,
    pub to_offset: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DepthShift {
    pub from_depth: i32,
    pub to_depth: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoreOffsetInsertReport {
    pub requested_offset: i32,
    pub actual_offset: i32,
    pub shifted_offsets: Vec<OffsetShift>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoreOffsetReplaceReport {
    pub offset: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoreOffsetRemoveReport {
    pub removed_offset: i32,
    pub shifted_offsets: Vec<OffsetShift>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DepthInsertReport {
    pub depth: i32,
    pub shifted_depths: Vec<DepthShift>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DepthReplaceReport {
    pub depth: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DepthRemoveReport {
    pub removed_depth: i32,
    pub shifted_depths: Vec<DepthShift>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoreOffsetArray<T> {
    core: Option<T>,
    pre: Vec<T>,
    post: Vec<T>,
}

impl<T> Default for CoreOffsetArray<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CoreOffsetArray<T> {
    pub fn new() -> Self {
        Self {
            core: None,
            pre: Vec::new(),
            post: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        usize::from(self.core.is_some()) + self.pre.len() + self.post.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, offset: i32) -> Option<&T> {
        match offset {
            0 => self.core.as_ref(),
            value if value < 0 => self.pre.get((-value - 1) as usize),
            value => self.post.get((value - 1) as usize),
        }
    }

    pub fn contains_offset(&self, offset: i32) -> bool {
        self.get(offset).is_some()
    }

    pub fn replace(
        &mut self,
        offset: i32,
        item: T,
    ) -> Result<CoreOffsetReplaceReport, PactArrayError> {
        match offset {
            0 => match self.core.as_mut() {
                Some(slot) => {
                    *slot = item;
                    Ok(CoreOffsetReplaceReport { offset })
                }
                None => Err(PactArrayError::InvalidReplacement {
                    coordinate: "0".to_string(),
                }),
            },
            value_offset if value_offset < 0 => {
                let index = (-value_offset - 1) as usize;
                match self.pre.get_mut(index) {
                    Some(slot) => {
                        *slot = item;
                        Ok(CoreOffsetReplaceReport { offset })
                    }
                    None => Err(PactArrayError::InvalidReplacement {
                        coordinate: offset.to_string(),
                    }),
                }
            }
            value_offset => {
                let index = (value_offset - 1) as usize;
                match self.post.get_mut(index) {
                    Some(slot) => {
                        *slot = item;
                        Ok(CoreOffsetReplaceReport { offset })
                    }
                    None => Err(PactArrayError::InvalidReplacement {
                        coordinate: offset.to_string(),
                    }),
                }
            }
        }
    }

    pub fn set(&mut self, offset: i32, item: T) -> Result<CoreOffsetReplaceReport, PactArrayError> {
        self.replace(offset, item)
    }

    pub fn insert(&mut self, requested_offset: i32, value: T) -> CoreOffsetInsertReport {
        let mut shifted_offsets = Vec::new();

        if requested_offset == 0 {
            if let Some(existing_core) = self.core.replace(value) {
                shifted_offsets.push(OffsetShift {
                    from_offset: 0,
                    to_offset: 1,
                });
                for index in 0..self.post.len() {
                    let from_offset = (index as i32) + 1;
                    shifted_offsets.push(OffsetShift {
                        from_offset,
                        to_offset: from_offset + 1,
                    });
                }
                self.post.insert(0, existing_core);
            }
            return CoreOffsetInsertReport {
                requested_offset,
                actual_offset: 0,
                shifted_offsets,
            };
        }

        if self.core.is_none() {
            self.core = Some(value);
            return CoreOffsetInsertReport {
                requested_offset,
                actual_offset: 0,
                shifted_offsets,
            };
        }

        let actual_offset = if requested_offset < 0 {
            let target_index = (-requested_offset - 1) as usize;
            if target_index <= self.pre.len() {
                for index in target_index..self.pre.len() {
                    let from_offset = -((index as i32) + 1);
                    shifted_offsets.push(OffsetShift {
                        from_offset,
                        to_offset: from_offset - 1,
                    });
                }
                self.pre.insert(target_index, value);
                requested_offset
            } else {
                self.pre.push(value);
                -(self.pre.len() as i32)
            }
        } else {
            let target_index = (requested_offset - 1) as usize;
            if target_index <= self.post.len() {
                for index in target_index..self.post.len() {
                    let from_offset = (index as i32) + 1;
                    shifted_offsets.push(OffsetShift {
                        from_offset,
                        to_offset: from_offset + 1,
                    });
                }
                self.post.insert(target_index, value);
                requested_offset
            } else {
                self.post.push(value);
                self.post.len() as i32
            }
        };

        CoreOffsetInsertReport {
            requested_offset,
            actual_offset,
            shifted_offsets,
        }
    }

    pub fn append(&mut self, value: T, requested_offset: i32) -> CoreOffsetInsertReport {
        let shifted_offsets = Vec::new();
        if self.core.is_none() {
            self.core = Some(value);
            return CoreOffsetInsertReport {
                requested_offset,
                actual_offset: 0,
                shifted_offsets,
            };
        }

        let actual_offset = if requested_offset < 0 {
            self.pre.push(value);
            -(self.pre.len() as i32)
        } else {
            self.post.push(value);
            self.post.len() as i32
        };

        CoreOffsetInsertReport {
            requested_offset,
            actual_offset,
            shifted_offsets,
        }
    }

    pub fn remove(&mut self, offset: i32) -> Result<(T, CoreOffsetRemoveReport), PactArrayError> {
        let mut shifted_offsets = Vec::new();
        match offset {
            0 => match self.core.take() {
                Some(removed) => {
                    if !self.post.is_empty() {
                        for index in 0..self.post.len() {
                            let from_offset = (index as i32) + 1;
                            shifted_offsets.push(OffsetShift {
                                from_offset,
                                to_offset: from_offset - 1,
                            });
                        }
                        self.core = Some(self.post.remove(0));
                    }
                    Ok((
                        removed,
                        CoreOffsetRemoveReport {
                            removed_offset: offset,
                            shifted_offsets,
                        },
                    ))
                }
                None => Err(PactArrayError::MissingCore),
            },
            value if value < 0 => {
                let index = (-value - 1) as usize;
                if index >= self.pre.len() {
                    return Err(PactArrayError::MissingOffset { offset });
                }
                for shifted_index in (index + 1)..self.pre.len() {
                    let from_offset = -((shifted_index as i32) + 1);
                    shifted_offsets.push(OffsetShift {
                        from_offset,
                        to_offset: from_offset + 1,
                    });
                }
                Ok((
                    self.pre.remove(index),
                    CoreOffsetRemoveReport {
                        removed_offset: offset,
                        shifted_offsets,
                    },
                ))
            }
            value => {
                let index = (value - 1) as usize;
                if index >= self.post.len() {
                    return Err(PactArrayError::MissingOffset { offset });
                }
                for shifted_index in (index + 1)..self.post.len() {
                    let from_offset = (shifted_index as i32) + 1;
                    shifted_offsets.push(OffsetShift {
                        from_offset,
                        to_offset: from_offset - 1,
                    });
                }
                Ok((
                    self.post.remove(index),
                    CoreOffsetRemoveReport {
                        removed_offset: offset,
                        shifted_offsets,
                    },
                ))
            }
        }
    }

    pub fn clear(&mut self) {
        self.core = None;
        self.pre.clear();
        self.post.clear();
    }

    pub fn offsets(&self) -> Vec<i32> {
        let mut offsets = Vec::with_capacity(self.len());
        for index in 0..self.pre.len() {
            offsets.push(-((index as i32) + 1));
        }
        if self.core.is_some() {
            offsets.push(0);
        }
        for index in 0..self.post.len() {
            offsets.push((index as i32) + 1);
        }
        offsets.sort();
        offsets
    }

    pub fn iter_ordered(&self) -> Vec<(i32, &T)> {
        let mut items = Vec::with_capacity(self.len());
        for (index, item) in self.pre.iter().enumerate() {
            items.push((-((index as i32) + 1), item));
        }
        if let Some(core) = &self.core {
            items.push((0, core));
        }
        for (index, item) in self.post.iter().enumerate() {
            items.push(((index as i32) + 1, item));
        }
        items.sort_by_key(|(offset, _)| *offset);
        items
    }

    pub fn ordered_node_ids(&self) -> Vec<(i32, T)>
    where
        T: Clone,
    {
        self.iter_ordered()
            .into_iter()
            .map(|(offset, value)| (offset, value.clone()))
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DepthArray<T> {
    depths: Vec<T>,
}

impl<T> Default for DepthArray<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> DepthArray<T> {
    pub fn new() -> Self {
        Self { depths: Vec::new() }
    }

    pub fn from_depth_map(items: BTreeMap<i32, T>) -> Result<Self, PactArrayError>
    where
        T: Default,
    {
        let mut out = Self::new();
        for (depth, item) in items {
            out.set(depth, item)?;
        }
        Ok(out)
    }

    pub fn index_for_depth(depth: i32) -> Result<usize, PactArrayError> {
        if depth < -1 {
            return Err(PactArrayError::InvalidDepth { depth });
        }
        Ok((depth + 1) as usize)
    }

    pub fn depth_for_index(index: usize) -> i32 {
        (index as i32) - 1
    }

    pub fn len(&self) -> usize {
        self.depths.len().saturating_sub(1)
    }

    pub fn storage_len(&self) -> usize {
        self.depths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.depths.is_empty()
    }

    pub fn max_depth(&self) -> i32 {
        if self.depths.is_empty() {
            -1
        } else {
            (self.depths.len() as i32) - 2
        }
    }

    pub fn get(&self, depth: i32) -> Option<&T> {
        let index = Self::index_for_depth(depth).ok()?;
        self.depths.get(index)
    }

    pub fn contains_depth(&self, depth: i32) -> bool {
        self.get(depth).is_some()
    }

    pub fn depths(&self) -> Vec<i32> {
        (0..self.depths.len()).map(Self::depth_for_index).collect()
    }

    pub fn set(&mut self, depth: i32, item: T) -> Result<DepthReplaceReport, PactArrayError>
    where
        T: Default,
    {
        let index = Self::index_for_depth(depth)?;
        while self.depths.len() <= index {
            self.depths.push(T::default());
        }
        self.depths[index] = item;
        Ok(DepthReplaceReport { depth })
    }

    pub fn replace(&mut self, depth: i32, item: T) -> Result<DepthReplaceReport, PactArrayError> {
        let index = Self::index_for_depth(depth)?;
        match self.depths.get_mut(index) {
            Some(slot) => {
                *slot = item;
                Ok(DepthReplaceReport { depth })
            }
            None => Err(PactArrayError::InvalidReplacement {
                coordinate: format!("d{depth}"),
            }),
        }
    }

    pub fn insert(&mut self, depth: i32, item: T) -> Result<DepthInsertReport, PactArrayError> {
        if depth <= 0 {
            return Err(PactArrayError::ProtectedDepthRemoval { depth });
        }
        let index = Self::index_for_depth(depth)?;
        if index > self.depths.len() {
            return Err(PactArrayError::SparseDepthInsert {
                depth,
                max_depth: self.max_depth(),
            });
        }
        let shifted_depths = (index..self.depths.len())
            .map(|storage_index| {
                let from_depth = Self::depth_for_index(storage_index);
                DepthShift {
                    from_depth,
                    to_depth: from_depth + 1,
                }
            })
            .collect();
        self.depths.insert(index, item);
        Ok(DepthInsertReport {
            depth,
            shifted_depths,
        })
    }

    pub fn append(&mut self, item: T) -> DepthInsertReport {
        self.depths.push(item);
        DepthInsertReport {
            depth: Self::depth_for_index(self.depths.len() - 1),
            shifted_depths: Vec::new(),
        }
    }

    pub fn remove(&mut self, depth: i32) -> Result<(T, DepthRemoveReport), PactArrayError> {
        if depth == -1 {
            return Err(PactArrayError::ProtectedDepthRemoval { depth });
        }
        if depth == 0 {
            return Err(PactArrayError::ProtectedDepthRemoval { depth });
        }
        let index = Self::index_for_depth(depth)?;
        if index >= self.depths.len() {
            return Err(PactArrayError::MissingDepth { depth });
        }
        let shifted_depths = ((index + 1)..self.depths.len())
            .map(|storage_index| {
                let from_depth = Self::depth_for_index(storage_index);
                DepthShift {
                    from_depth,
                    to_depth: from_depth - 1,
                }
            })
            .collect();
        Ok((
            self.depths.remove(index),
            DepthRemoveReport {
                removed_depth: depth,
                shifted_depths,
            },
        ))
    }

    pub fn clear(&mut self) {
        self.depths.clear();
    }

    pub fn iter_depth_items(&self) -> impl Iterator<Item = (i32, &T)> {
        self.depths
            .iter()
            .enumerate()
            .map(|(index, item)| (Self::depth_for_index(index), item))
    }

    pub fn ordered_node_ids(&self) -> Vec<(i32, T)>
    where
        T: Clone,
    {
        self.iter_depth_items()
            .map(|(depth, value)| (depth, value.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests;
