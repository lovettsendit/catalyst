//! Dominators, dominance frontiers and natural loops.
//!
//! Needed three times over: `mem2reg` places phis on the dominance frontier,
//! global value numbering may only reuse a value whose definition dominates
//! the use, and loop-invariant code motion needs to know what a loop is. Each
//! of those is a correctness requirement rather than a nicety -- a GVN that
//! skips the dominance check produces a program that reads a register which
//! was never written on the path taken.
//!
//! Cooper, Harvey and Kennedy's iterative formulation, which converges in a
//! couple of passes over reverse post-order on any CFG a real program has.

use crate::ir::{BlockId, Func};
use std::collections::{HashMap, HashSet};

pub struct DomTree {
    /// Immediate dominator of each block; the entry is its own.
    pub idom: Vec<BlockId>,
    /// Children in the dominator tree.
    pub children: Vec<Vec<BlockId>>,
    /// Euler in/out numbers, so `dominates` is two comparisons.
    tin: Vec<u32>,
    tout: Vec<u32>,
    pub reachable: Vec<bool>,
}

impl DomTree {
    pub fn build(f: &Func) -> DomTree {
        let n = f.blocks.len();
        let rpo = f.rpo();
        let mut order = vec![u32::MAX; n];
        for (i, &b) in rpo.iter().enumerate() {
            order[b as usize] = i as u32;
        }
        let preds = f.preds();

        let undef = BlockId::MAX;
        let mut idom = vec![undef; n];
        idom[f.entry as usize] = f.entry;

        let intersect = |mut a: BlockId, mut b: BlockId, idom: &Vec<BlockId>| -> BlockId {
            while a != b {
                while order[a as usize] > order[b as usize] {
                    a = idom[a as usize];
                }
                while order[b as usize] > order[a as usize] {
                    b = idom[b as usize];
                }
            }
            a
        };

        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter() {
                if b == f.entry {
                    continue;
                }
                let mut new: Option<BlockId> = None;
                for &p in &preds[b as usize] {
                    if idom[p as usize] == undef {
                        continue;
                    }
                    new = Some(match new {
                        None => p,
                        Some(cur) => intersect(p, cur, &idom),
                    });
                }
                if let Some(new) = new {
                    if idom[b as usize] != new {
                        idom[b as usize] = new;
                        changed = true;
                    }
                }
            }
        }

        let mut reachable = vec![false; n];
        for &b in &rpo {
            reachable[b as usize] = true;
        }

        let mut children = vec![Vec::new(); n];
        for &b in &rpo {
            if b != f.entry && idom[b as usize] != undef {
                children[idom[b as usize] as usize].push(b);
            }
        }

        // Euler tour of the dominator tree, iteratively.
        let mut tin = vec![0u32; n];
        let mut tout = vec![0u32; n];
        let mut clock = 0u32;
        let mut stack = vec![(f.entry, 0usize)];
        tin[f.entry as usize] = clock;
        clock += 1;
        while let Some((b, i)) = stack.pop() {
            if i < children[b as usize].len() {
                stack.push((b, i + 1));
                let c = children[b as usize][i];
                tin[c as usize] = clock;
                clock += 1;
                stack.push((c, 0));
            } else {
                tout[b as usize] = clock;
                clock += 1;
            }
        }

        DomTree {
            idom,
            children,
            tin,
            tout,
            reachable,
        }
    }

    /// Does `a` dominate `b`? Reflexive: a block dominates itself.
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if !self.reachable[a as usize] || !self.reachable[b as usize] {
            return false;
        }
        self.tin[a as usize] <= self.tin[b as usize]
            && self.tout[b as usize] <= self.tout[a as usize]
    }

    /// The dominance frontier: for each block, the blocks where its dominance
    /// stops. Exactly where a phi is needed for a definition placed in it.
    pub fn frontiers(&self, f: &Func) -> Vec<HashSet<BlockId>> {
        let preds = f.preds();
        let mut df = vec![HashSet::new(); f.blocks.len()];
        for b in 0..f.blocks.len() as BlockId {
            if preds[b as usize].len() < 2 || !self.reachable[b as usize] {
                continue;
            }
            for &p in &preds[b as usize] {
                let mut runner = p;
                while runner != self.idom[b as usize] && self.reachable[runner as usize] {
                    df[runner as usize].insert(b);
                    let next = self.idom[runner as usize];
                    if next == runner {
                        break;
                    }
                    runner = next;
                }
            }
        }
        df
    }
}

/// A natural loop: a header, the back edges into it, and every block inside.
#[derive(Debug, Clone)]
pub struct Loop {
    pub header: BlockId,
    pub latches: Vec<BlockId>,
    pub body: HashSet<BlockId>,
}

/// Every natural loop, innermost first.
///
/// A back edge is an edge `latch -> header` whose target dominates its source;
/// the body is everything that reaches the latch without leaving through the
/// header. Irreducible control flow has no back edge by this definition and is
/// simply not treated as a loop, which costs an optimisation and never
/// correctness.
pub fn loops(f: &Func, dt: &DomTree) -> Vec<Loop> {
    let preds = f.preds();
    let mut by_header: HashMap<BlockId, Loop> = HashMap::new();
    for b in 0..f.blocks.len() as BlockId {
        if !dt.reachable[b as usize] {
            continue;
        }
        for s in f.succs(b) {
            if dt.dominates(s, b) {
                let entry = by_header.entry(s).or_insert_with(|| Loop {
                    header: s,
                    latches: Vec::new(),
                    body: HashSet::from([s]),
                });
                entry.latches.push(b);
                // Backwards flood from the latch, stopping at the header.
                let mut stack = vec![b];
                while let Some(x) = stack.pop() {
                    if !entry.body.insert(x) {
                        continue;
                    }
                    for &p in &preds[x as usize] {
                        stack.push(p);
                    }
                }
            }
        }
    }
    let mut out: Vec<Loop> = by_header.into_values().collect();
    // Innermost first, so that hoisting out of an inner loop happens before
    // the outer one considers the same instruction.
    out.sort_by_key(|l| l.body.len());
    out
}
