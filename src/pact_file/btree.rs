use std::collections::BTreeMap;

use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

pub(crate) const PAGE_BYTES: usize = 768;
const PAGE_HEADER_BYTES: usize = 64;
const PAGE_MAGIC: &[u8; 4] = b"PBT2";
const PAGE_VERSION: u8 = 3;
const INLINE_VALUE_LIMIT: usize = 512;
const OVERFLOW_CHUNK_BYTES: usize = PAGE_BYTES - PAGE_HEADER_BYTES - 8;
const MAX_VALUE_BYTES: usize = 64 * 1024 * 1024;

type PageAddress = u64;
pub(crate) type RangeWindow = (usize, Vec<(Vec<u8>, Vec<u8>)>);

pub(crate) trait PageReader {
    fn read_page(&self, address: PageAddress) -> PactResult<[u8; PAGE_BYTES]>;

    fn read_component_payload(&self, _batch_address: u64, _ordinal: u32) -> PactResult<Vec<u8>> {
        Err(PactError::unsupported(
            "Pact page reader cannot resolve canonical component payloads",
        ))
    }
}

pub(crate) trait PageWriter: PageReader {
    fn write_page(&mut self, page: [u8; PAGE_BYTES]) -> PactResult<PageAddress>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WriteStats {
    pub(crate) pages_written: u64,
}

#[derive(Default)]
pub(crate) struct PageStore {
    pages: BTreeMap<PageAddress, [u8; PAGE_BYTES]>,
    next_address: PageAddress,
}

impl PageStore {
    pub(crate) fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub(crate) fn page_bytes(&self) -> usize {
        self.pages.len().saturating_mul(PAGE_BYTES)
    }
}

impl PageReader for PageStore {
    fn read_page(&self, address: PageAddress) -> PactResult<[u8; PAGE_BYTES]> {
        self.pages
            .get(&address)
            .copied()
            .ok_or_else(|| PactError::corrupt(format!("missing Pact B+tree page at {address}")))
    }
}

impl PageWriter for PageStore {
    fn write_page(&mut self, page: [u8; PAGE_BYTES]) -> PactResult<PageAddress> {
        let address = if self.next_address == 0 {
            PAGE_BYTES as u64
        } else {
            self.next_address
        };
        self.next_address = address
            .checked_add(PAGE_BYTES as u64)
            .ok_or_else(|| PactError::invalid("Pact B+tree page address overflow"))?;
        self.pages.insert(address, page);
        Ok(address)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CowBTree {
    root: Option<PageAddress>,
}

impl CowBTree {
    pub(crate) fn from_root(root: Option<PageAddress>) -> Self {
        Self { root }
    }

    pub(crate) fn bulk_load(
        pages: &mut impl PageWriter,
        mut entries: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> PactResult<Self> {
        if entries.is_empty() {
            return Ok(Self::default());
        }
        if entries.iter().any(|(key, _)| key.is_empty()) {
            return Err(PactError::invalid("Pact B+tree keys cannot be empty"));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut unique = Vec::<(Vec<u8>, Vec<u8>)>::with_capacity(entries.len());
        for entry in entries {
            if unique
                .last()
                .is_some_and(|(existing, _)| existing == &entry.0)
            {
                if let Some(last) = unique.last_mut() {
                    *last = entry;
                }
            } else {
                unique.push(entry);
            }
        }

        let unique = unique
            .into_iter()
            .map(|(key, value)| Ok((key, store_value(pages, value, None)?)))
            .collect::<PactResult<Vec<_>>>()?;
        let mut level = Vec::new();
        let mut cursor = 0;
        while cursor < unique.len() {
            let mut leaf = Vec::new();
            while cursor < unique.len() {
                leaf.push(unique[cursor].clone());
                if !page_fits(&Node::Leaf {
                    entries: leaf.clone(),
                }) {
                    leaf.pop();
                    break;
                }
                cursor += 1;
            }
            if leaf.is_empty() {
                return Err(PactError::invalid("Pact B+tree key/value exceeds one page"));
            }
            level.push(write_node(pages, &Node::Leaf { entries: leaf })?);
        }

        while level.len() > 1 {
            let mut next = Vec::new();
            let mut cursor = 0;
            while cursor < level.len() {
                let mut children = Vec::new();
                while cursor < level.len() {
                    children.push(level[cursor]);
                    let node = internal_from_children(pages, children.clone())?;
                    if !page_fits(&node) {
                        children.pop();
                        break;
                    }
                    cursor += 1;
                }
                if children.is_empty() {
                    return Err(PactError::invalid(
                        "Pact B+tree internal entry exceeds page size",
                    ));
                }
                let node = internal_from_children(pages, children)?;
                next.push(write_node(pages, &node)?);
            }
            level = next;
        }
        Ok(Self {
            root: level.first().copied(),
        })
    }

    pub(crate) fn root_address(&self) -> Option<PageAddress> {
        self.root
    }

    pub(crate) fn bulk_load_sorted(
        pages: &mut impl PageWriter,
        entries: impl IntoIterator<Item = PactResult<(Vec<u8>, Vec<u8>)>>,
    ) -> PactResult<Self> {
        let mut leaf: Vec<(Vec<u8>, StoredValue)> = Vec::new();
        let mut levels: Vec<Vec<PageAddress>> = Vec::new();
        let mut last_key: Option<Vec<u8>> = None;
        for entry in entries {
            let (key, value) = entry?;
            if key.is_empty() {
                return Err(PactError::invalid("Pact B+tree keys cannot be empty"));
            }
            let stored = store_value(pages, value, None)?;
            match last_key.as_ref().map(|last| key.cmp(last)) {
                Some(std::cmp::Ordering::Less) => {
                    return Err(PactError::invalid(
                        "Pact B+tree bulk load requires sorted keys",
                    ));
                }
                Some(std::cmp::Ordering::Equal) => {
                    if let Some(last) = leaf.last_mut() {
                        last.1 = stored;
                    }
                    continue;
                }
                _ => {}
            }
            last_key = Some(key.clone());
            leaf.push((key, stored));
            if !page_fits(&Node::Leaf {
                entries: leaf.clone(),
            }) {
                let overflow = leaf
                    .pop()
                    .ok_or_else(|| PactError::invalid("Pact B+tree key/value exceeds one page"))?;
                if leaf.is_empty() {
                    return Err(PactError::invalid("Pact B+tree key/value exceeds one page"));
                }
                let address = write_node(
                    pages,
                    &Node::Leaf {
                        entries: std::mem::take(&mut leaf),
                    },
                )?;
                push_bulk_child(pages, &mut levels, 0, address)?;
                leaf.push(overflow);
            }
        }
        if leaf.is_empty() {
            return Ok(Self::default());
        }
        let address = write_node(pages, &Node::Leaf { entries: leaf })?;
        push_bulk_child(pages, &mut levels, 0, address)?;

        let mut level = 0;
        loop {
            let children = std::mem::take(&mut levels[level]);
            let is_top = levels.len() == level + 1;
            if is_top && children.len() == 1 {
                return Ok(Self {
                    root: children.first().copied(),
                });
            }
            if children.is_empty() {
                return Err(PactError::internal("Pact B+tree bulk load lost a level"));
            }
            let node = internal_from_children(pages, children)?;
            let address = write_node(pages, &node)?;
            push_bulk_child(pages, &mut levels, level + 1, address)?;
            level += 1;
        }
    }

    pub(crate) fn insert(
        &self,
        pages: &mut impl PageWriter,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> PactResult<(Self, WriteStats)> {
        if key.is_empty() {
            return Err(PactError::invalid("Pact B+tree keys cannot be empty"));
        }
        let mut stats = WriteStats::default();
        let value = store_value(pages, value, Some(&mut stats))?;
        let root = if let Some(root) = self.root {
            let inserted = insert_node(pages, root, key, value, &mut stats)?;
            if let Some((separator, right, right_count)) = inserted.split {
                let root = Node::Internal {
                    separators: vec![separator],
                    child_counts: vec![subtree_len(pages, inserted.address)?, right_count],
                    children: vec![inserted.address, right],
                };
                stats.pages_written += 1;
                write_node(pages, &root)?
            } else {
                inserted.address
            }
        } else {
            let root = Node::Leaf {
                entries: vec![(key, value)],
            };
            stats.pages_written += 1;
            write_node(pages, &root)?
        };
        Ok((Self { root: Some(root) }, stats))
    }

    pub(crate) fn get(
        &self,
        pages: &(impl PageReader + ?Sized),
        key: &[u8],
    ) -> PactResult<Option<Vec<u8>>> {
        let Some(mut address) = self.root else {
            return Ok(None);
        };
        loop {
            match read_node(pages, address)? {
                Node::Leaf { entries } => {
                    return entries
                        .binary_search_by(|(candidate, _)| candidate.as_slice().cmp(key))
                        .ok()
                        .map(|index| read_value(pages, &entries[index].1))
                        .transpose();
                }
                Node::Internal {
                    separators,
                    children,
                    ..
                } => {
                    let index = separators.partition_point(|separator| separator.as_slice() <= key);
                    address = *children.get(index).ok_or_else(|| {
                        PactError::corrupt("Pact B+tree internal child index is invalid")
                    })?;
                }
                Node::Overflow { .. } => {
                    return Err(PactError::corrupt(
                        "Pact B+tree root path reached an overflow page",
                    ));
                }
            }
        }
    }

    pub(crate) fn delete(
        &self,
        pages: &mut impl PageWriter,
        key: &[u8],
    ) -> PactResult<(Self, WriteStats, Option<Vec<u8>>)> {
        let Some(root) = self.root else {
            return Ok((self.clone(), WriteStats::default(), None));
        };
        let mut stats = WriteStats::default();
        let deleted = delete_node(pages, root, key, &mut stats)?;
        if deleted.removed.is_none() {
            return Ok((self.clone(), WriteStats::default(), None));
        }
        let removed = deleted
            .removed
            .as_ref()
            .map(|value| read_value(pages, value))
            .transpose()?;

        let mut root = deleted.address;
        while let Some(address) = root {
            match read_node(pages, address)? {
                Node::Internal { children, .. } if children.len() == 1 => {
                    root = children.first().copied();
                }
                _ => break,
            }
        }
        Ok((Self { root }, stats, removed))
    }

    pub(crate) fn range(
        &self,
        pages: &(impl PageReader + ?Sized),
        start: &[u8],
        end: &[u8],
    ) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        if start > end {
            return Err(PactError::invalid("Pact B+tree range start is after end"));
        }
        let mut entries = Vec::new();
        if let Some(root) = self.root {
            collect_range(pages, root, start, end, &mut entries)?;
        }
        Ok(entries)
    }

    pub(crate) fn range_limit(
        &self,
        pages: &(impl PageReader + ?Sized),
        start: &[u8],
        end: &[u8],
        limit: usize,
        reverse: bool,
    ) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        if start > end {
            return Err(PactError::invalid("Pact B+tree range start is after end"));
        }
        let mut entries = Vec::with_capacity(limit);
        if limit > 0
            && let Some(root) = self.root
        {
            collect_range_limit(pages, root, start, end, limit, reverse, &mut entries)?;
        }
        Ok(entries)
    }

    pub(crate) fn range_window(
        &self,
        pages: &(impl PageReader + ?Sized),
        start: &[u8],
        end: &[u8],
        offset: usize,
        limit: usize,
    ) -> PactResult<RangeWindow> {
        if start > end {
            return Err(PactError::invalid("Pact B+tree range start is after end"));
        }
        let Some(root) = self.root else {
            return Ok((0, Vec::new()));
        };
        let start_rank = rank_before(pages, root, start)?;
        let end_rank = rank_before(pages, root, end)?;
        let total = end_rank.saturating_sub(start_rank);
        if limit == 0 || offset >= total {
            return Ok((total, Vec::new()));
        }
        let window_start = start_rank.saturating_add(offset);
        let window_end = window_start.saturating_add(limit).min(end_rank);
        let mut entries = Vec::with_capacity(window_end.saturating_sub(window_start));
        collect_rank_window(pages, root, 0, window_start, window_end, &mut entries)?;
        Ok((total, entries))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Leaf {
        entries: Vec<(Vec<u8>, StoredValue)>,
    },
    Internal {
        separators: Vec<Vec<u8>>,
        children: Vec<PageAddress>,
        child_counts: Vec<u64>,
    },
    Overflow {
        next: Option<PageAddress>,
        chunk: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StoredValue {
    Inline(Vec<u8>),
    Overflow { address: PageAddress, len: usize },
}

impl StoredValue {
    fn len(&self) -> usize {
        match self {
            Self::Inline(value) => value.len(),
            Self::Overflow { len, .. } => *len,
        }
    }
}

struct InsertedNode {
    address: PageAddress,

    added: bool,

    split: Option<(Vec<u8>, PageAddress, u64)>,
}

struct DeletedNode {
    address: Option<PageAddress>,
    removed: Option<StoredValue>,
}

fn delete_node(
    pages: &mut impl PageWriter,
    address: PageAddress,
    key: &[u8],
    stats: &mut WriteStats,
) -> PactResult<DeletedNode> {
    match read_node(pages, address)? {
        Node::Leaf { mut entries } => {
            let Ok(index) =
                entries.binary_search_by(|(candidate, _)| candidate.as_slice().cmp(key))
            else {
                return Ok(DeletedNode {
                    address: Some(address),
                    removed: None,
                });
            };
            let removed = entries.remove(index).1;
            let address = if entries.is_empty() {
                None
            } else {
                stats.pages_written += 1;
                Some(write_node(pages, &Node::Leaf { entries })?)
            };
            Ok(DeletedNode {
                address,
                removed: Some(removed),
            })
        }
        Node::Internal {
            mut separators,
            mut children,
            mut child_counts,
        } => {
            let child_index = separators.partition_point(|separator| separator.as_slice() <= key);
            let child_address = *children
                .get(child_index)
                .ok_or_else(|| PactError::corrupt("Pact B+tree internal child index is invalid"))?;
            let deleted = delete_node(pages, child_address, key, stats)?;
            if deleted.removed.is_none() {
                return Ok(DeletedNode {
                    address: Some(address),
                    removed: None,
                });
            }
            if child_counts.len() != children.len() || separators.len() + 1 != children.len() {
                return Err(PactError::corrupt(
                    "Pact B+tree internal node is inconsistent",
                ));
            }

            if let Some(replacement) = deleted.address {
                children[child_index] = replacement;
                child_counts[child_index] = child_counts[child_index].saturating_sub(1);
            } else {
                children.remove(child_index);
                child_counts.remove(child_index);
                separators.remove(child_index.saturating_sub(1));
            }
            if children.is_empty() {
                return Ok(DeletedNode {
                    address: None,
                    removed: deleted.removed,
                });
            }
            merge_adjacent_children(
                pages,
                &mut separators,
                &mut children,
                &mut child_counts,
                child_index.saturating_sub(1),
                stats,
            )?;
            let node = Node::Internal {
                separators,
                children,
                child_counts,
            };
            stats.pages_written += 1;
            Ok(DeletedNode {
                address: Some(write_node(pages, &node)?),
                removed: deleted.removed,
            })
        }
        Node::Overflow { .. } => Err(PactError::corrupt(
            "Pact B+tree root path reached an overflow page",
        )),
    }
}

fn merge_adjacent_children(
    pages: &mut impl PageWriter,
    separators: &mut Vec<Vec<u8>>,
    children: &mut Vec<PageAddress>,
    child_counts: &mut Vec<u64>,
    left_index: usize,
    stats: &mut WriteStats,
) -> PactResult<()> {
    if left_index + 1 >= children.len() {
        return Ok(());
    }
    let left = read_node(pages, children[left_index])?;
    let right = read_node(pages, children[left_index + 1])?;
    let merged = match (left, right) {
        (Node::Leaf { mut entries }, Node::Leaf { entries: right }) => {
            entries.extend(right);
            Node::Leaf { entries }
        }
        (
            Node::Internal {
                separators: mut left_separators,
                children: mut left_children,
                child_counts: mut left_counts,
            },
            Node::Internal {
                separators: right_separators,
                children: right_children,
                child_counts: right_counts,
            },
        ) => {
            left_separators.push(separators[left_index].clone());
            left_separators.extend(right_separators);
            left_children.extend(right_children);
            left_counts.extend(right_counts);
            Node::Internal {
                separators: left_separators,
                children: left_children,
                child_counts: left_counts,
            }
        }
        _ => return Err(PactError::corrupt("Pact B+tree child levels do not match")),
    };
    if !page_fits(&merged) {
        return Ok(());
    }
    stats.pages_written += 1;
    children[left_index] = write_node(pages, &merged)?;
    children.remove(left_index + 1);
    let right_count = child_counts.remove(left_index + 1);
    child_counts[left_index] = child_counts[left_index]
        .checked_add(right_count)
        .ok_or_else(|| PactError::corrupt("Pact B+tree subtree count overflow"))?;
    separators.remove(left_index);
    Ok(())
}

fn push_bulk_child(
    pages: &mut impl PageWriter,
    levels: &mut Vec<Vec<PageAddress>>,
    level: usize,
    address: PageAddress,
) -> PactResult<()> {
    if levels.len() <= level {
        levels.resize_with(level + 1, Vec::new);
    }
    levels[level].push(address);
    let node = internal_from_children(pages, levels[level].clone())?;
    if page_fits(&node) {
        return Ok(());
    }
    let overflow = levels[level]
        .pop()
        .ok_or_else(|| PactError::internal("Pact B+tree bulk level lost a child"))?;
    if levels[level].is_empty() {
        return Err(PactError::invalid(
            "Pact B+tree internal entry exceeds page size",
        ));
    }
    let node = internal_from_children(pages, std::mem::take(&mut levels[level]))?;
    let written = write_node(pages, &node)?;
    levels[level].push(overflow);
    push_bulk_child(pages, levels, level + 1, written)
}

fn internal_from_children(
    pages: &(impl PageReader + ?Sized),
    children: Vec<PageAddress>,
) -> PactResult<Node> {
    let mut separators = Vec::with_capacity(children.len().saturating_sub(1));
    let mut child_counts = Vec::with_capacity(children.len());
    for child in &children {
        child_counts.push(subtree_len(pages, *child)?);
    }
    for child in children.iter().skip(1) {
        separators.push(first_key(pages, *child)?);
    }
    Ok(Node::Internal {
        separators,
        children,
        child_counts,
    })
}

fn insert_node(
    pages: &mut impl PageWriter,
    address: PageAddress,
    key: Vec<u8>,
    value: StoredValue,
    stats: &mut WriteStats,
) -> PactResult<InsertedNode> {
    match read_node(pages, address)? {
        Node::Leaf { mut entries } => {
            let added = match entries.binary_search_by(|(candidate, _)| candidate.cmp(&key)) {
                Ok(index) => {
                    entries[index].1 = value;
                    false
                }
                Err(index) => {
                    entries.insert(index, (key, value));
                    true
                }
            };
            loop {
                let node = Node::Leaf {
                    entries: entries.clone(),
                };
                if page_fits(&node) {
                    stats.pages_written += 1;
                    return Ok(InsertedNode {
                        address: write_node(pages, &node)?,
                        added,
                        split: None,
                    });
                }
                if entries.len() < 2 {
                    return Err(PactError::invalid("Pact B+tree key/value exceeds one page"));
                }
                let midpoint = entries.len() / 2;
                let split_at = (1..entries.len())
                    .filter(|index| {
                        page_fits(&Node::Leaf {
                            entries: entries[..*index].to_vec(),
                        }) && page_fits(&Node::Leaf {
                            entries: entries[*index..].to_vec(),
                        })
                    })
                    .min_by_key(|index| index.abs_diff(midpoint));
                if let Some(split_at) = split_at {
                    let right_entries = entries.split_off(split_at);
                    let separator = right_entries
                        .first()
                        .map(|(key, _)| key.clone())
                        .ok_or_else(|| {
                            PactError::internal("Pact B+tree split produced empty leaf")
                        })?;
                    let right_count = right_entries.len() as u64;
                    let left = Node::Leaf { entries };
                    let right = Node::Leaf {
                        entries: right_entries,
                    };
                    stats.pages_written += 2;
                    let left_address = write_node(pages, &left)?;
                    let right_address = write_node(pages, &right)?;
                    return Ok(InsertedNode {
                        address: left_address,
                        added,
                        split: Some((separator, right_address, right_count)),
                    });
                }

                let Some((index, value)) = entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (_, value))| match value {
                        StoredValue::Inline(value) => Some((index, value)),
                        StoredValue::Overflow { .. } => None,
                    })
                    .max_by_key(|(_, value)| value.len())
                    .map(|(index, value)| (index, value.clone()))
                else {
                    let largest_entry = entries
                        .iter()
                        .map(|(key, value)| (key.len(), value.len()))
                        .max_by_key(|(key_len, value_len)| key_len.saturating_add(*value_len))
                        .unwrap_or_default();
                    return Err(PactError::invalid(format!(
                        "Pact B+tree split entries exceed page size (largest key={} value={})",
                        largest_entry.0, largest_entry.1
                    )));
                };
                entries[index].1 = store_overflow_value(pages, value, Some(stats))?;
            }
        }
        Node::Internal {
            mut separators,
            mut children,
            mut child_counts,
        } => {
            let child_index =
                separators.partition_point(|separator| separator.as_slice() <= key.as_slice());
            let child_address = *children
                .get(child_index)
                .ok_or_else(|| PactError::corrupt("Pact B+tree internal child index is invalid"))?;
            let inserted = insert_node(pages, child_address, key, value, stats)?;

            children[child_index] = inserted.address;
            if child_index > 0 {
                separators[child_index - 1] = first_key(pages, inserted.address)?;
            }
            let count = child_counts
                .get_mut(child_index)
                .ok_or_else(|| PactError::corrupt("Pact B+tree child count index is invalid"))?;
            if inserted.added {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| PactError::corrupt("Pact B+tree subtree count overflow"))?;
            }
            if let Some((separator, right, right_count)) = inserted.split {
                *count = count.checked_sub(right_count).ok_or_else(|| {
                    PactError::corrupt("Pact B+tree split count exceeds its subtree")
                })?;
                separators.insert(child_index, separator);
                children.insert(child_index + 1, right);
                child_counts.insert(child_index + 1, right_count);
            }
            let node = Node::Internal {
                separators,
                children,
                child_counts,
            };
            if page_fits(&node) {
                stats.pages_written += 1;
                return Ok(InsertedNode {
                    address: write_node(pages, &node)?,
                    added: inserted.added,
                    split: None,
                });
            }
            let Node::Internal {
                mut separators,
                mut children,
                mut child_counts,
            } = node
            else {
                return Err(PactError::internal(
                    "Pact B+tree internal node changed kind",
                ));
            };
            if children.len() < 4 {
                return Err(PactError::invalid(
                    "Pact B+tree internal entry exceeds page size",
                ));
            }
            let split_child = children.len() / 2;
            let right_children = children.split_off(split_child);
            let right_counts = child_counts.split_off(split_child);
            let promoted = separators
                .get(split_child - 1)
                .cloned()
                .ok_or_else(|| PactError::internal("Pact B+tree missing promoted separator"))?;
            let right_separators = separators.split_off(split_child);
            separators.truncate(split_child - 1);
            let right_count = right_counts
                .iter()
                .try_fold(0_u64, |total, count| total.checked_add(*count))
                .ok_or_else(|| PactError::corrupt("Pact B+tree subtree count overflow"))?;
            let left = Node::Internal {
                separators,
                children,
                child_counts,
            };
            let right = Node::Internal {
                separators: right_separators,
                children: right_children,
                child_counts: right_counts,
            };
            stats.pages_written += 2;
            let left_address = write_node(pages, &left)?;
            let right_address = write_node(pages, &right)?;
            Ok(InsertedNode {
                address: left_address,
                added: inserted.added,
                split: Some((promoted, right_address, right_count)),
            })
        }
        Node::Overflow { .. } => Err(PactError::corrupt(
            "Pact B+tree root path reached an overflow page",
        )),
    }
}

fn first_key(pages: &(impl PageReader + ?Sized), mut address: PageAddress) -> PactResult<Vec<u8>> {
    loop {
        match read_node(pages, address)? {
            Node::Leaf { entries } => {
                return entries
                    .first()
                    .map(|(key, _)| key.clone())
                    .ok_or_else(|| PactError::corrupt("Pact B+tree leaf is empty"));
            }
            Node::Internal { children, .. } => {
                address = *children
                    .first()
                    .ok_or_else(|| PactError::corrupt("Pact B+tree internal node is empty"))?;
            }
            Node::Overflow { .. } => {
                return Err(PactError::corrupt(
                    "Pact B+tree root path reached an overflow page",
                ));
            }
        }
    }
}

fn subtree_len(pages: &(impl PageReader + ?Sized), address: PageAddress) -> PactResult<u64> {
    match read_node(pages, address)? {
        Node::Leaf { entries } => u64::try_from(entries.len())
            .map_err(|_| PactError::corrupt("Pact B+tree leaf count overflow")),
        Node::Internal { child_counts, .. } => child_counts
            .into_iter()
            .try_fold(0_u64, |total, count| total.checked_add(count))
            .ok_or_else(|| PactError::corrupt("Pact B+tree subtree count overflow")),
        Node::Overflow { .. } => Err(PactError::corrupt(
            "Pact B+tree structural path reached an overflow page",
        )),
    }
}

fn rank_before(
    pages: &(impl PageReader + ?Sized),
    mut address: PageAddress,
    key: &[u8],
) -> PactResult<usize> {
    let mut rank = 0_u64;
    loop {
        match read_node(pages, address)? {
            Node::Leaf { entries } => {
                let within = entries.partition_point(|(candidate, _)| candidate.as_slice() < key);
                rank = rank
                    .checked_add(
                        u64::try_from(within)
                            .map_err(|_| PactError::corrupt("Pact B+tree leaf rank exceeds u64"))?,
                    )
                    .ok_or_else(|| PactError::corrupt("Pact B+tree rank overflow"))?;
                return usize::try_from(rank)
                    .map_err(|_| PactError::corrupt("Pact B+tree rank exceeds platform limits"));
            }
            Node::Internal {
                separators,
                children,
                child_counts,
            } => {
                if children.len() != child_counts.len() {
                    return Err(PactError::corrupt("Pact B+tree child/count mismatch"));
                }
                let index = separators.partition_point(|separator| separator.as_slice() <= key);
                rank = child_counts[..index]
                    .iter()
                    .try_fold(rank, |total, count| {
                        total
                            .checked_add(*count)
                            .ok_or_else(|| PactError::corrupt("Pact B+tree rank overflow"))
                    })?;
                address = *children.get(index).ok_or_else(|| {
                    PactError::corrupt("Pact B+tree internal child index is invalid")
                })?;
            }
            Node::Overflow { .. } => {
                return Err(PactError::corrupt(
                    "Pact B+tree rank reached an overflow page",
                ));
            }
        }
    }
}

fn collect_rank_window(
    pages: &(impl PageReader + ?Sized),
    address: PageAddress,
    base_rank: usize,
    start_rank: usize,
    end_rank: usize,
    out: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> PactResult<()> {
    if start_rank >= end_rank {
        return Ok(());
    }
    match read_node(pages, address)? {
        Node::Leaf { entries } => {
            let local_start = start_rank.saturating_sub(base_rank).min(entries.len());
            let local_end = end_rank.saturating_sub(base_rank).min(entries.len());
            for (key, value) in entries
                .into_iter()
                .skip(local_start)
                .take(local_end.saturating_sub(local_start))
            {
                out.push((key, read_value(pages, &value)?));
            }
        }
        Node::Internal {
            children,
            child_counts,
            ..
        } => {
            if children.len() != child_counts.len() {
                return Err(PactError::corrupt("Pact B+tree child/count mismatch"));
            }
            let mut child_base = base_rank;
            for (child, count) in children.into_iter().zip(child_counts) {
                let count = usize::try_from(count).map_err(|_| {
                    PactError::corrupt("Pact B+tree child count exceeds platform limits")
                })?;
                let child_end = child_base
                    .checked_add(count)
                    .ok_or_else(|| PactError::corrupt("Pact B+tree rank overflow"))?;
                if child_end > start_rank && child_base < end_rank {
                    collect_rank_window(pages, child, child_base, start_rank, end_rank, out)?;
                }
                if child_end >= end_rank {
                    break;
                }
                child_base = child_end;
            }
        }
        Node::Overflow { .. } => {
            return Err(PactError::corrupt(
                "Pact B+tree rank window reached an overflow page",
            ));
        }
    }
    Ok(())
}

fn collect_range(
    pages: &(impl PageReader + ?Sized),
    address: PageAddress,
    start: &[u8],
    end: &[u8],
    out: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> PactResult<()> {
    match read_node(pages, address)? {
        Node::Leaf { entries } => {
            for (key, value) in entries
                .into_iter()
                .filter(|(key, _)| key.as_slice() >= start && key.as_slice() < end)
            {
                out.push((key, read_value(pages, &value)?));
            }
        }
        Node::Internal {
            separators,
            children,
            ..
        } => {
            for (index, child) in children.into_iter().enumerate() {
                let lower = index.checked_sub(1).and_then(|at| separators.get(at));
                let upper = separators.get(index);
                let overlaps = upper.is_none_or(|upper| upper.as_slice() > start)
                    && lower.is_none_or(|lower| lower.as_slice() < end);
                if overlaps {
                    collect_range(pages, child, start, end, out)?;
                }
            }
        }
        Node::Overflow { .. } => {
            return Err(PactError::corrupt(
                "Pact B+tree range reached an overflow page",
            ));
        }
    }
    Ok(())
}

fn collect_range_limit(
    pages: &(impl PageReader + ?Sized),
    address: PageAddress,
    start: &[u8],
    end: &[u8],
    limit: usize,
    reverse: bool,
    out: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> PactResult<()> {
    if out.len() >= limit {
        return Ok(());
    }
    match read_node(pages, address)? {
        Node::Leaf { entries } => {
            let mut matching = entries
                .into_iter()
                .filter(|(key, _)| key.as_slice() >= start && key.as_slice() < end)
                .collect::<Vec<_>>();
            if reverse {
                matching.reverse();
            }
            for (key, value) in matching.into_iter().take(limit - out.len()) {
                out.push((key, read_value(pages, &value)?));
            }
        }
        Node::Overflow { .. } => {
            return Err(PactError::corrupt(
                "Pact B+tree range reached an overflow page",
            ));
        }
        Node::Internal {
            separators,
            children,
            ..
        } => {
            let indices: Box<dyn Iterator<Item = usize>> = if reverse {
                Box::new((0..children.len()).rev())
            } else {
                Box::new(0..children.len())
            };
            for index in indices {
                let lower = index.checked_sub(1).and_then(|at| separators.get(at));
                let upper = separators.get(index);
                let overlaps = upper.is_none_or(|upper| upper.as_slice() > start)
                    && lower.is_none_or(|lower| lower.as_slice() < end);
                if overlaps {
                    collect_range_limit(pages, children[index], start, end, limit, reverse, out)?;
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

fn read_node(pages: &(impl PageReader + ?Sized), address: PageAddress) -> PactResult<Node> {
    decode_page(&pages.read_page(address)?)
}

fn write_node(pages: &mut impl PageWriter, node: &Node) -> PactResult<PageAddress> {
    pages.write_page(encode_page(node)?)
}

fn store_value(
    pages: &mut impl PageWriter,
    value: Vec<u8>,
    stats: Option<&mut WriteStats>,
) -> PactResult<StoredValue> {
    if value.len() <= INLINE_VALUE_LIMIT {
        return Ok(StoredValue::Inline(value));
    }
    store_overflow_value(pages, value, stats)
}

fn store_overflow_value(
    pages: &mut impl PageWriter,
    value: Vec<u8>,
    mut stats: Option<&mut WriteStats>,
) -> PactResult<StoredValue> {
    if value.len() > MAX_VALUE_BYTES {
        return Err(PactError::invalid(format!(
            "Pact B+tree value exceeds {MAX_VALUE_BYTES} bytes"
        )));
    }
    let len = value.len();
    let mut next = None;
    for chunk in value.chunks(OVERFLOW_CHUNK_BYTES).rev() {
        let address = write_node(
            pages,
            &Node::Overflow {
                next,
                chunk: chunk.to_vec(),
            },
        )?;
        if let Some(stats) = stats.as_deref_mut() {
            stats.pages_written += 1;
        }
        next = Some(address);
    }
    Ok(StoredValue::Overflow {
        address: next.ok_or_else(|| PactError::internal("empty Pact overflow value"))?,
        len,
    })
}

fn read_value(pages: &(impl PageReader + ?Sized), value: &StoredValue) -> PactResult<Vec<u8>> {
    let StoredValue::Overflow { address, len } = value else {
        let StoredValue::Inline(value) = value else {
            unreachable!();
        };
        return Ok(value.clone());
    };
    if *len > MAX_VALUE_BYTES {
        return Err(PactError::corrupt(
            "Pact overflow value exceeds its fixed limit",
        ));
    }
    let mut out = Vec::with_capacity(*len);
    let mut next = Some(*address);
    let max_pages = len.div_ceil(OVERFLOW_CHUNK_BYTES).saturating_add(1);
    let mut pages_seen = 0;
    while let Some(address) = next {
        pages_seen += 1;
        if pages_seen > max_pages {
            return Err(PactError::corrupt(
                "Pact overflow page chain contains a cycle",
            ));
        }
        let Node::Overflow {
            next: following,
            chunk,
        } = read_node(pages, address)?
        else {
            return Err(PactError::corrupt(
                "Pact overflow address references a structural page",
            ));
        };
        if out.len().saturating_add(chunk.len()) > *len {
            return Err(PactError::corrupt(
                "Pact overflow value exceeds declared length",
            ));
        }
        out.extend_from_slice(&chunk);
        next = following;
    }
    if out.len() != *len {
        return Err(PactError::corrupt(
            "Pact overflow value does not match declared length",
        ));
    }
    Ok(out)
}

pub(crate) fn map_page_references(
    page: &[u8; PAGE_BYTES],
    mut map: impl FnMut(u64) -> PactResult<u64>,
) -> PactResult<[u8; PAGE_BYTES]> {
    match decode_page(page)? {
        Node::Leaf { mut entries } => {
            for (_, value) in &mut entries {
                if let StoredValue::Overflow { address, .. } = value {
                    *address = map(*address)?;
                }
            }
            encode_page(&Node::Leaf { entries })
        }
        Node::Internal {
            separators,
            mut children,
            child_counts,
        } => {
            for child in &mut children {
                *child = map(*child)?;
            }
            encode_page(&Node::Internal {
                separators,
                children,
                child_counts,
            })
        }
        Node::Overflow { mut next, chunk } => {
            if let Some(address) = &mut next {
                *address = map(*address)?;
            }
            encode_page(&Node::Overflow { next, chunk })
        }
    }
}

pub(crate) fn page_references(page: &[u8; PAGE_BYTES]) -> PactResult<Vec<u64>> {
    Ok(match decode_page(page)? {
        Node::Leaf { entries } => entries
            .into_iter()
            .filter_map(|(_, value)| match value {
                StoredValue::Overflow { address, .. } => Some(address),
                StoredValue::Inline(_) => None,
            })
            .collect(),
        Node::Internal { children, .. } => children,
        Node::Overflow { next, .. } => next.into_iter().collect(),
    })
}

fn page_fits(node: &Node) -> bool {
    encode_page(node).is_ok()
}

fn encode_page(node: &Node) -> PactResult<[u8; PAGE_BYTES]> {
    let mut payload = Vec::new();
    let (kind, count) = match node {
        Node::Leaf { entries } => {
            let mut previous = Vec::<u8>::new();
            for (key, value) in entries {
                let prefix = common_prefix(&previous, key);
                let suffix = &key[prefix..];
                let prefix = u16::try_from(prefix)
                    .map_err(|_| PactError::invalid("Pact B+tree key prefix is too long"))?;
                let suffix_len = u16::try_from(suffix.len())
                    .map_err(|_| PactError::invalid("Pact B+tree key suffix is too long"))?;
                payload.extend_from_slice(&prefix.to_le_bytes());
                payload.extend_from_slice(&suffix_len.to_le_bytes());
                payload.extend_from_slice(suffix);
                match value {
                    StoredValue::Inline(value) => {
                        payload.push(0);
                        let value_len = u32::try_from(value.len())
                            .map_err(|_| PactError::invalid("Pact B+tree value is too long"))?;
                        payload.extend_from_slice(&value_len.to_le_bytes());
                        payload.extend_from_slice(value);
                    }
                    StoredValue::Overflow { address, len } => {
                        payload.push(1);
                        payload.extend_from_slice(
                            &u64::try_from(*len)
                                .map_err(|_| {
                                    PactError::invalid("Pact B+tree value length overflow")
                                })?
                                .to_le_bytes(),
                        );
                        payload.extend_from_slice(&address.to_le_bytes());
                    }
                }
                previous = key.clone();
            }
            (0_u8, entries.len())
        }
        Node::Internal {
            separators,
            children,
            child_counts,
        } => {
            if children.len() != separators.len() + 1 || children.len() != child_counts.len() {
                return Err(PactError::internal(
                    "Pact B+tree internal child/separator/count mismatch",
                ));
            }
            let first = children
                .first()
                .ok_or_else(|| PactError::invalid("Pact B+tree internal node is empty"))?;
            payload.extend_from_slice(&first.to_le_bytes());
            payload.extend_from_slice(&child_counts[0].to_le_bytes());
            for ((separator, child), child_count) in separators
                .iter()
                .zip(&children[1..])
                .zip(&child_counts[1..])
            {
                let len = u16::try_from(separator.len())
                    .map_err(|_| PactError::invalid("Pact B+tree separator is too long"))?;
                payload.extend_from_slice(&len.to_le_bytes());
                payload.extend_from_slice(separator);
                payload.extend_from_slice(&child.to_le_bytes());
                payload.extend_from_slice(&child_count.to_le_bytes());
            }
            (1_u8, separators.len())
        }
        Node::Overflow { next, chunk } => {
            payload.extend_from_slice(&next.unwrap_or_default().to_le_bytes());
            payload.extend_from_slice(chunk);
            (2_u8, chunk.len())
        }
    };
    if payload.len() > PAGE_BYTES - PAGE_HEADER_BYTES {
        return Err(PactError::invalid("Pact B+tree node exceeds page size"));
    }
    let count = u16::try_from(count)
        .map_err(|_| PactError::invalid("Pact B+tree page has too many entries"))?;
    let payload_len = u16::try_from(payload.len())
        .map_err(|_| PactError::invalid("Pact B+tree payload length overflow"))?;
    let mut page = [0_u8; PAGE_BYTES];
    page[..4].copy_from_slice(PAGE_MAGIC);
    page[4] = kind;
    page[5] = PAGE_VERSION;
    page[6..8].copy_from_slice(&count.to_le_bytes());
    page[8..10].copy_from_slice(&payload_len.to_le_bytes());
    page[PAGE_HEADER_BYTES..PAGE_HEADER_BYTES + payload.len()].copy_from_slice(&payload);
    let digest = page_digest(&page, payload.len());
    page[16..20].copy_from_slice(&digest);
    Ok(page)
}

fn decode_page(page: &[u8; PAGE_BYTES]) -> PactResult<Node> {
    if &page[..4] != PAGE_MAGIC || page[5] != PAGE_VERSION {
        return Err(PactError::corrupt("invalid Pact B+tree page header"));
    }
    let count = usize::from(u16::from_le_bytes([page[6], page[7]]));
    let payload_len = usize::from(u16::from_le_bytes([page[8], page[9]]));
    if payload_len > PAGE_BYTES - PAGE_HEADER_BYTES
        || page_digest(page, payload_len) != page[16..20]
    {
        return Err(PactError::corrupt("Pact B+tree page checksum mismatch"));
    }
    let payload = &page[PAGE_HEADER_BYTES..PAGE_HEADER_BYTES + payload_len];
    let mut cursor = 0;
    let node = match page[4] {
        0 => {
            let mut entries = Vec::with_capacity(count);
            let mut previous = Vec::<u8>::new();
            for _ in 0..count {
                let prefix = usize::from(read_u16(payload, &mut cursor)?);
                let suffix_len = usize::from(read_u16(payload, &mut cursor)?);
                if prefix > previous.len() {
                    return Err(PactError::corrupt("invalid Pact B+tree key prefix"));
                }
                let mut key = previous[..prefix].to_vec();
                key.extend_from_slice(take(payload, &mut cursor, suffix_len)?);
                let kind = *take(payload, &mut cursor, 1)?
                    .first()
                    .ok_or_else(|| PactError::corrupt("missing Pact B+tree value kind"))?;
                let value = match kind {
                    0 => {
                        let value_len = usize::try_from(read_u32(payload, &mut cursor)?)
                            .map_err(|_| PactError::corrupt("Pact B+tree value length overflow"))?;
                        StoredValue::Inline(take(payload, &mut cursor, value_len)?.to_vec())
                    }
                    1 => {
                        let len =
                            usize::try_from(read_u64(payload, &mut cursor)?).map_err(|_| {
                                PactError::corrupt(
                                    "Pact B+tree overflow length exceeds platform limits",
                                )
                            })?;
                        let address = read_u64(payload, &mut cursor)?;
                        if len > MAX_VALUE_BYTES || address == 0 {
                            return Err(PactError::corrupt(
                                "invalid Pact B+tree overflow reference",
                            ));
                        }
                        StoredValue::Overflow { address, len }
                    }
                    _ => return Err(PactError::corrupt("unknown Pact B+tree value kind")),
                };
                previous = key.clone();
                entries.push((key, value));
            }
            Node::Leaf { entries }
        }
        1 => {
            let mut children = vec![read_u64(payload, &mut cursor)?];
            let mut child_counts = vec![read_u64(payload, &mut cursor)?];
            let mut separators = Vec::with_capacity(count);
            for _ in 0..count {
                let len = usize::from(read_u16(payload, &mut cursor)?);
                separators.push(take(payload, &mut cursor, len)?.to_vec());
                children.push(read_u64(payload, &mut cursor)?);
                child_counts.push(read_u64(payload, &mut cursor)?);
            }
            if child_counts.contains(&0) {
                return Err(PactError::corrupt(
                    "Pact B+tree internal child has zero entries",
                ));
            }
            Node::Internal {
                separators,
                children,
                child_counts,
            }
        }
        2 => {
            let raw_next = read_u64(payload, &mut cursor)?;
            let chunk = payload[cursor..].to_vec();
            cursor = payload.len();
            if chunk.len() != count || chunk.len() > OVERFLOW_CHUNK_BYTES {
                return Err(PactError::corrupt("invalid Pact overflow page length"));
            }
            Node::Overflow {
                next: (raw_next != 0).then_some(raw_next),
                chunk,
            }
        }
        kind => {
            return Err(PactError::corrupt(format!(
                "unknown Pact B+tree page kind: {kind}"
            )));
        }
    };
    if cursor != payload.len() {
        return Err(PactError::corrupt(
            "Pact B+tree page has trailing payload bytes",
        ));
    }
    Ok(node)
}

fn page_digest(page: &[u8; PAGE_BYTES], payload_len: usize) -> [u8; 4] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&page[..16]);
    hasher.update(&page[PAGE_HEADER_BYTES..PAGE_HEADER_BYTES + payload_len]);
    hasher.finalize().as_bytes()[..4]
        .try_into()
        .expect("fixed Pact B+tree digest width")
}

fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> PactResult<u16> {
    let raw = take(bytes, cursor, 2)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> PactResult<u32> {
    let raw = take(bytes, cursor, 4)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> PactResult<u64> {
    let raw = take(bytes, cursor, 8)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> PactResult<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| PactError::corrupt("Pact B+tree payload length overrun"))?;
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}
