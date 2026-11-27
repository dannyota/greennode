use std::collections::VecDeque;
use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
struct Node {
    child: u32,
    sibling: u32,
    failure: u32,
    output: u32,
    length: u32,
    byte: u8,
    flags: u8,
}

impl Zeroize for Node {
    fn zeroize(&mut self) {
        self.child.zeroize();
        self.sibling.zeroize();
        self.failure.zeroize();
        self.output.zeroize();
        self.length.zeroize();
        self.byte.zeroize();
        self.flags.zeroize();
    }
}

pub(super) struct Automaton {
    roots: Zeroizing<[u32; 256]>,
    nodes: Zeroizing<Vec<Node>>,
}

pub(super) fn charge(work: &mut usize, amount: usize) -> Option<()> {
    *work = work.checked_sub(amount)?;
    Some(())
}

impl Automaton {
    pub(super) fn build<'a>(
        patterns: impl Iterator<Item = (&'a [u8], u8)> + Clone,
        bytes: &mut usize,
        work: &mut usize,
    ) -> Option<Self> {
        let capacity = patterns.clone().try_fold(1usize, |total, (value, _)| {
            charge(bytes, value.len())?;
            total.checked_add(value.len())
        })?;
        let mut matcher = Self {
            roots: Zeroizing::new([0; 256]),
            nodes: Zeroizing::new(Vec::with_capacity(capacity)),
        };
        matcher.nodes.push(Node::default());
        for (pattern, flags) in patterns {
            let mut state = 0;
            for &byte in pattern {
                charge(work, 1)?;
                let mut child = matcher.child(state, byte, work)?;
                if child == 0 {
                    child = u32::try_from(matcher.nodes.len()).ok()?;
                    let parent = &mut matcher.nodes[state as usize];
                    let sibling = if state == 0 {
                        matcher.roots[usize::from(byte)] = child;
                        0
                    } else {
                        std::mem::replace(&mut parent.child, child)
                    };
                    matcher.nodes.push(Node {
                        sibling,
                        byte,
                        ..Node::default()
                    });
                }
                state = child;
            }
            let terminal = &mut matcher.nodes[state as usize];
            terminal.length = u32::try_from(pattern.len()).ok()?;
            terminal.flags |= flags;
        }
        matcher.link_suffixes(work)?;
        Some(matcher)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.nodes.len() == 1
    }

    fn child(&self, state: u32, byte: u8, work: &mut usize) -> Option<u32> {
        if state == 0 {
            return Some(self.roots[usize::from(byte)]);
        }
        let mut child = self.nodes[state as usize].child;
        while child != 0 {
            let edge = &self.nodes[child as usize];
            if edge.byte == byte {
                return Some(child);
            }
            child = edge.sibling;
            if child != 0 {
                charge(work, 1)?;
            }
        }
        Some(0)
    }

    fn link_suffixes(&mut self, work: &mut usize) -> Option<()> {
        let mut queue: VecDeque<_> = self
            .roots
            .iter()
            .copied()
            .filter(|node| *node != 0)
            .collect();
        while let Some(parent) = queue.pop_front() {
            let mut child = self.nodes[parent as usize].child;
            let failure = self.nodes[parent as usize].failure;
            while child != 0 {
                charge(work, 1)?;
                let fallback = self.advance(failure, self.nodes[child as usize].byte, work)?;
                let suffix = &self.nodes[fallback as usize];
                let output = if suffix.flags == 0 {
                    suffix.output
                } else {
                    fallback
                };
                let node = &mut self.nodes[child as usize];
                node.failure = fallback;
                node.output = output;
                queue.push_back(child);
                child = node.sibling;
            }
        }
        Some(())
    }

    pub(super) fn advance(&self, mut state: u32, byte: u8, work: &mut usize) -> Option<u32> {
        loop {
            let next = self.child(state, byte, work)?;
            if next != 0 || state == 0 {
                return Some(next);
            }
            charge(work, 1)?;
            state = self.nodes[state as usize].failure;
        }
    }

    pub(super) fn accepts(
        &self,
        mut state: u32,
        work: &mut usize,
        accepts: impl Fn(usize, u8) -> bool,
    ) -> Option<bool> {
        if state == 0 {
            return Some(false);
        }
        let node = &self.nodes[state as usize];
        if node.flags == 0 {
            state = node.output;
        }
        while state != 0 {
            charge(work, 1)?;
            let node = &self.nodes[state as usize];
            if accepts(node.length as usize, node.flags) {
                return Some(true);
            }
            state = node.output;
        }
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_matches_keep_suffix_candidates_and_follow_fallbacks() {
        let patterns = [(b"she".as_slice(), 1), (b"he", 2), (b"hers", 4)];
        let mut work = 128;
        let matcher = Automaton::build(patterns.into_iter(), &mut 64, &mut work).unwrap();
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
