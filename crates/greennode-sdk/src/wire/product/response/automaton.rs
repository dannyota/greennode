use std::collections::VecDeque;
use zeroize::{Zeroize, Zeroizing};

pub(super) const SCAN_BLOCK: usize = 24 << 10;

#[derive(Default)]
struct Node {
    child: u32,
    sibling_byte: u32,
    failure: u32,
    output: u32,
}

impl Node {
    const MASK: u32 = 0x00ff_ffff;

    fn sibling(&self) -> u32 {
        self.sibling_byte & Self::MASK
    }
    fn byte(&self) -> u8 {
        (self.sibling_byte >> 24) as u8
    }
    fn flags(&self) -> u8 {
        (self.child >> 24) as u8
    }
    fn child(&self) -> u32 {
        self.child & Self::MASK
    }
    fn failure(&self) -> u32 {
        self.failure & Self::MASK
    }
    fn output(&self) -> u32 {
        self.output & Self::MASK
    }
    fn short_length(&self) -> u16 {
        (((self.output >> 24) << 8) | (self.failure >> 24)) as u16
    }
}

impl Zeroize for Node {
    fn zeroize(&mut self) {
        self.child.zeroize();
        self.sibling_byte.zeroize();
        self.failure.zeroize();
        self.output.zeroize();
    }
}

const NODE_CHUNK: usize = 4096;

struct NodeArena {
    first: Zeroizing<Vec<Node>>,
    chunks: Vec<Zeroizing<Vec<Node>>>,
    len: usize,
    capacity: usize,
}

impl NodeArena {
    fn new(first: Vec<Node>) -> Self {
        let capacity = first.capacity();
        Self {
            first: Zeroizing::new(first),
            chunks: Vec::new(),
            len: 0,
            capacity,
        }
    }

    fn len(&self) -> usize {
        self.len
    }
    fn capacity(&self) -> usize {
        self.capacity
    }

    fn push(&mut self, node: Node) -> Option<()> {
        if self.len < NODE_CHUNK {
            self.first.push(node);
        } else {
            self.chunks.get_mut(self.len / NODE_CHUNK - 1)?.push(node);
        }
        self.len += 1;
        Some(())
    }

    fn grow(&mut self, bytes: &mut usize, lowest: &mut usize) -> Option<()> {
        if !self.capacity.is_multiple_of(NODE_CHUNK) {
            return None;
        }
        if self.chunks.len() == self.chunks.capacity() {
            let old = self.chunks.capacity();
            let previous = old.checked_mul(size_of::<Zeroizing<Vec<Node>>>())?;
            let growth = old.clamp(1, 64);
            charge(bytes, previous)?;
            charge(
                bytes,
                growth.checked_mul(size_of::<Zeroizing<Vec<Node>>>())?,
            )?;
            *lowest = (*lowest).min(*bytes);
            self.chunks.try_reserve_exact(growth).ok()?;
            charge(
                bytes,
                (self.chunks.capacity() - old - growth)
                    .checked_mul(size_of::<Zeroizing<Vec<Node>>>())?,
            )?;
            *lowest = (*lowest).min(*bytes);
            *bytes = bytes.checked_add(previous)?;
        }
        let queue = self.len.saturating_sub(1).checked_mul(size_of::<u32>())?;
        let capacity =
            NODE_CHUNK.min(bytes.saturating_sub(queue) / (size_of::<Node>() + size_of::<u32>()));
        if capacity == 0 {
            return None;
        }
        charge(bytes, capacity.checked_mul(size_of::<Node>())?)?;
        *lowest = (*lowest).min(*bytes);
        let mut chunk = Vec::new();
        chunk.try_reserve_exact(capacity).ok()?;
        charge(
            bytes,
            (chunk.capacity() - capacity).checked_mul(size_of::<Node>())?,
        )?;
        *lowest = (*lowest).min(*bytes);
        self.capacity = self.capacity.checked_add(chunk.capacity())?;
        self.chunks.push(Zeroizing::new(chunk));
        Some(())
    }
}

impl std::ops::Index<usize> for NodeArena {
    type Output = Node;
    fn index(&self, index: usize) -> &Node {
        if index < NODE_CHUNK {
            &self.first[index]
        } else {
            &self.chunks[index / NODE_CHUNK - 1][index % NODE_CHUNK]
        }
    }
}
impl std::ops::IndexMut<usize> for NodeArena {
    fn index_mut(&mut self, index: usize) -> &mut Node {
        if index < NODE_CHUNK {
            &mut self.first[index]
        } else {
            &mut self.chunks[index / NODE_CHUNK - 1][index % NODE_CHUNK]
        }
    }
}

pub(super) struct Automaton {
    roots: Zeroizing<[u32; 256]>,
    nodes: NodeArena,
    terminals: Zeroizing<Vec<(u32, u32)>>,
}

pub(super) fn charge(work: &mut usize, amount: usize) -> Option<()> {
    *work = work.checked_sub(amount)?;
    Some(())
}

impl Automaton {
    #[cfg(test)]
    pub(super) fn build<P: AsRef<[u8]>>(
        patterns: impl Iterator<Item = (P, u8)> + Clone,
        bytes: &mut usize,
        work: &mut usize,
    ) -> Option<Self> {
        let mut lowest = *bytes;
        Self::build_with_peak(patterns, bytes, work, &mut lowest)
    }

    pub(super) fn build_with_peak<P: AsRef<[u8]>>(
        patterns: impl Iterator<Item = (P, u8)> + Clone,
        bytes: &mut usize,
        work: &mut usize,
        lowest: &mut usize,
    ) -> Option<Self> {
        let upper = patterns.clone().try_fold(1usize, |sum, (pattern, _)| {
            charge(work, pattern.as_ref().len())?;
            sum.checked_add(pattern.as_ref().len())
        })?;
        charge(bytes, size_of::<[u32; 256]>())?;
        let mut nodes = Vec::new();
        let required = upper
            .checked_mul(size_of::<Node>())?
            .checked_add(upper.saturating_sub(1).checked_mul(size_of::<u32>())?)?;
        let capacity = if upper <= 1024 && required <= *bytes {
            upper
        } else {
            NODE_CHUNK.min(*bytes / (size_of::<Node>() + size_of::<u32>()))
        };
        if capacity == 0 {
            return None;
        }
        charge(bytes, capacity.checked_mul(size_of::<Node>())?)?;
        nodes.try_reserve_exact(capacity).ok()?;
        charge(
            bytes,
            (nodes.capacity() - capacity).checked_mul(size_of::<Node>())?,
        )?;
        *lowest = (*lowest).min(*bytes);
        let mut matcher = Self {
            roots: Zeroizing::new([0; 256]),
            nodes: NodeArena::new(nodes),
            terminals: Zeroizing::new(Vec::new()),
        };
        matcher.nodes.push(Node::default())?;
        for (pattern, flags) in patterns {
            let pattern = pattern.as_ref();
            let mut state = 0;
            for &byte in pattern {
                charge(work, 1)?;
                let mut child = matcher.child(state, byte, work)?;
                if child == 0 {
                    if matcher.nodes.len() == matcher.nodes.capacity() {
                        matcher.nodes.grow(bytes, lowest)?;
                    }
                    child = u32::try_from(matcher.nodes.len()).ok()?;
                    if child > Node::MASK {
                        return None;
                    }
                    let parent = &mut matcher.nodes[state as usize];
                    let sibling = if state == 0 {
                        matcher.roots[usize::from(byte)] = child;
                        0
                    } else {
                        let sibling = parent.child();
                        parent.child = (parent.child & !Node::MASK) | child;
                        sibling
                    };
                    matcher.nodes.push(Node {
                        sibling_byte: sibling | (u32::from(byte) << 24),
                        ..Node::default()
                    })?;
                }
                state = child;
            }
            let length = u32::try_from(pattern.len()).ok()?;
            if length >= u32::from(u16::MAX) && matcher.nodes[state as usize].flags() == 0 {
                if matcher.terminals.len() == matcher.terminals.capacity() {
                    let old = matcher.terminals.capacity();
                    let previous_bytes = old.checked_mul(size_of::<(u32, u32)>())?;
                    let growth = old
                        .clamp(1, 1024)
                        .min(bytes.saturating_sub(previous_bytes) / size_of::<(u32, u32)>());
                    if growth == 0 {
                        return None;
                    }
                    charge(bytes, previous_bytes)?;
                    charge(bytes, growth.checked_mul(size_of::<(u32, u32)>())?)?;
                    *lowest = (*lowest).min(*bytes);
                    matcher.terminals.try_reserve_exact(growth).ok()?;
                    charge(
                        bytes,
                        (matcher.terminals.capacity() - old - growth).checked_mul(size_of::<(
                            u32,
                            u32,
                        )>(
                        ))?,
                    )?;
                    *lowest = (*lowest).min(*bytes);
                    *bytes = bytes.checked_add(previous_bytes)?;
                }
                matcher.terminals.push((state, length));
            }
            let node = &mut matcher.nodes[state as usize];
            let short = length.min(u32::from(u16::MAX));
            node.failure = (node.failure & Node::MASK) | ((short & 0xff) << 24);
            node.output = (node.output & Node::MASK) | ((short >> 8) << 24);
            node.child |= u32::from(flags) << 24;
        }
        matcher.terminals.sort_unstable_by_key(|(state, _)| *state);
        matcher.link_suffixes(bytes, work, lowest)?;
        *lowest = (*lowest).min(*bytes);
        Some(matcher)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.nodes.len() == 1
    }

    pub(super) fn starts(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=u8::MAX).filter(|&byte| self.root(byte) != 0)
    }

    pub(super) fn root(&self, byte: u8) -> u32 {
        self.roots[usize::from(byte)]
    }

    fn child(&self, state: u32, byte: u8, work: &mut usize) -> Option<u32> {
        if state == 0 {
            return Some(self.root(byte));
        }
        let mut child = self.nodes[state as usize].child();
        while child != 0 {
            let edge = &self.nodes[child as usize];
            if edge.byte() == byte {
                return Some(child);
            }
            child = edge.sibling();
            if child != 0 {
                charge(work, 1)?;
            }
        }
        Some(0)
    }

    fn link_suffixes(
        &mut self,
        bytes: &mut usize,
        work: &mut usize,
        lowest: &mut usize,
    ) -> Option<()> {
        let capacity = self.nodes.len() - 1;
        charge(bytes, capacity.checked_mul(size_of::<u32>())?)?;
        let mut queue = VecDeque::new();
        queue.try_reserve_exact(capacity).ok()?;
        charge(
            bytes,
            (queue.capacity() - capacity).checked_mul(size_of::<u32>())?,
        )?;
        let allocated = queue.capacity().checked_mul(size_of::<u32>())?;
        *lowest = (*lowest).min(*bytes);
        queue.extend(self.roots.iter().copied().filter(|node| *node != 0));
        while let Some(parent) = queue.pop_front() {
            let mut child = self.nodes[parent as usize].child();
            let failure = self.nodes[parent as usize].failure();
            while child != 0 {
                charge(work, 1)?;
                let fallback = self.advance(failure, self.nodes[child as usize].byte(), work)?;
                let suffix = &self.nodes[fallback as usize];
                let output = if suffix.flags() == 0 {
                    suffix.output()
                } else {
                    fallback
                };
                let node = &mut self.nodes[child as usize];
                node.failure = (node.failure & !Node::MASK) | fallback;
                node.output = (node.output & !Node::MASK) | output;
                queue.push_back(child);
                child = node.sibling();
            }
        }
        drop(queue);
        *bytes = bytes.checked_add(allocated)?;
        Some(())
    }

    pub(super) fn advance(&self, mut state: u32, byte: u8, work: &mut usize) -> Option<u32> {
        loop {
            let next = self.child(state, byte, work)?;
            if next != 0 || state == 0 {
                return Some(next);
            }
            charge(work, 1)?;
            state = self.nodes[state as usize].failure();
        }
    }

    pub(super) fn contains(&self, input: &[u8], work: &mut usize) -> Option<bool> {
        charge(work, input.len())?;
        let starts =
            (input.len() >= SCAN_BLOCK).then(|| Zeroizing::new(self.starts().collect::<Vec<_>>()));
        let mut state = 0;
        for block in input.chunks(SCAN_BLOCK) {
            if state == 0 && block_excludes_starts(block, starts.as_deref().map(Vec::as_slice)) {
                continue;
            }
            for &byte in block {
                state = if state == 0 {
                    self.root(byte)
                } else {
                    self.advance(state, byte, work)?
                };
                if state != 0 && self.accepts(state, work, |_, _| true)? {
                    return Some(true);
                }
            }
        }
        Some(false)
    }

    pub(super) fn accepts(
        &self,
        mut state: u32,
        work: &mut usize,
        mut accepts: impl FnMut(usize, u8) -> bool,
    ) -> Option<bool> {
        if state == 0 {
            return Some(false);
        }
        let node = &self.nodes[state as usize];
        if node.flags() == 0 {
            state = node.output();
        }
        while state != 0 {
            charge(work, 1)?;
            let node = &self.nodes[state as usize];
            let short = node.short_length();
            let length = if short == u16::MAX {
                let position = self
                    .terminals
                    .binary_search_by_key(&state, |(id, _)| *id)
                    .ok()?;
                self.terminals.get(position)?.1 as usize
            } else {
                usize::from(short)
            };
            if accepts(length, node.flags()) {
                return Some(true);
            }
            state = node.output();
        }
        Some(false)
    }
}

pub(super) fn block_excludes_starts(block: &[u8], starts: Option<&[u8]>) -> bool {
    starts.is_some_and(|bytes| bytes.len() <= 16 && !bytes.iter().any(|byte| block.contains(byte)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_budgets_shared_prefixes_once() {
        let values: Vec<_> = (0..128)
            .map(|i| format!("{}-{i}", "x".repeat(1024)))
            .collect();
        let patterns = values.iter().map(|value| (value.as_bytes(), 1));
        assert!(Automaton::build(patterns, &mut (1 << 20), &mut (8 << 20)).is_some());
    }

    #[test]
    fn storage_budget_covers_reserved_nodes_root_transitions_and_suffix_queue() {
        let pattern = b"abcd".as_slice();
        let node_capacity = pattern.len() + 1;
        let root_bytes = size_of::<[u32; 256]>();
        let node_bytes = node_capacity * size_of::<Node>();
        let queue_bytes = pattern.len() * size_of::<u32>();
        let total = root_bytes + node_bytes + queue_bytes;
        assert!(Automaton::build([(pattern, 1)].into_iter(), &mut (total - 1), &mut 128).is_none());
        let mut budget = total;
        let mut lowest = total;
        let matcher = Automaton::build_with_peak(
            [(pattern, 1)].into_iter(),
            &mut budget,
            &mut 128,
            &mut lowest,
        )
        .unwrap();
        assert_eq!(matcher.nodes.capacity(), node_capacity);
        assert_eq!(lowest, 0);
        assert_eq!(budget, queue_bytes);
    }

    #[test]
    fn overlapping_matches_keep_suffix_candidates_and_follow_fallbacks() {
        let patterns = [(b"she".as_slice(), 1), (b"he", 2), (b"hers", 4)];
        let mut work = 128;
        let matcher = Automaton::build(patterns.into_iter(), &mut (4 << 10), &mut work).unwrap();
        let state = b"she".iter().fold(0, |state, byte| {
            matcher.advance(state, *byte, &mut work).unwrap()
        });
        assert_eq!(
            matcher.accepts(state, &mut work, |length, flags| (length, flags) == (2, 2)),
            Some(true)
        );
        let state = b"rs".iter().fold(state, |state, byte| {
            matcher.advance(state, *byte, &mut work).unwrap()
        });
        assert_eq!(
            matcher.accepts(state, &mut work, |length, flags| (length, flags) == (4, 4)),
            Some(true)
        );
        assert_eq!(matcher.accepts(state, &mut 0, |_, _| false), None);
    }
}
