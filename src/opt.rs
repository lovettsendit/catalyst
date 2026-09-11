//! The optimiser that runs *before* differentiation.
//!
//! This file is the reason the project exists. Enzyme's measured claim is that
//! differentiating optimised IR beats differentiating unoptimised IR by about
//! 4.2x, and that claim is only testable if there is a real optimiser here to
//! turn on and off. So these are the passes that actually change what AD has
//! to do:
//!
//! * [`mem2reg`] is the big one. Before it, a scalar living in an `alloca` is
//!   a memory location, and reverse mode has to give it a *shadow* location,
//!   shadow stores and shadow loads. After it, the same scalar is an SSA value
//!   whose adjoint is a register. Memory traffic in the gradient collapses.
//! * [`fold`] removes arithmetic that AD would otherwise have to differentiate
//!   -- and every folded multiply is a multiply plus two adjoint multiplies
//!   that never get generated.
//! * [`gvn`] makes a repeated subexpression one value, so its adjoint is
//!   accumulated once instead of recomputed per use.
//! * [`licm`] moves invariants out of loops, so their adjoints leave too.
//! * [`dce`] deletes what is left, both before AD and again after it.
//!
//! Run the same pipeline again on the differentiated function and it cleans up
//! the transform's own leavings -- the zero seeds, the `x + 0.0` accumulations
//! into fresh adjoints, the cached values nothing reads.

use crate::domtree::{loops, DomTree};
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// How much of the pipeline to run. The benchmarks compare `None` against
/// `Full` with everything else held fixed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    /// Nothing. The IR as it was built.
    None,
    /// Folding and dead code only: the cheap wins, no memory promotion.
    Light,
    /// Everything.
    Full,
}

/// What a run of the pipeline did, so the logs can say so with numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptStats {
    pub folded: usize,
    pub cse: usize,
    pub promoted_allocas: usize,
    pub hoisted: usize,
    pub deleted: usize,
    pub rounds: usize,
    pub before: usize,
    pub after: usize,
}

/// Run the pipeline to a fixpoint.
pub fn optimise(module: &mut Module, id: FuncId, level: Level) -> OptStats {
    let mut stats = OptStats {
        before: module.get(id).inst_count(),
        ..Default::default()
    };
    if level == Level::None {
        stats.after = stats.before;
        return stats;
    }
    for round in 0..8 {
        stats.rounds = round + 1;
        let mut moved = 0;
        moved += fold(module, id, &mut stats);
        if level == Level::Full {
            moved += mem2reg(module, id, &mut stats);
            moved += gvn(module, id, &mut stats);
            moved += licm(module, id, &mut stats);
        }
        moved += dce(module, id, &mut stats);
        if moved == 0 {
            break;
        }
    }
    stats.after = module.get(id).inst_count();
    stats
}

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

/// Rewrite every operand *and terminator operand* through `map`, then drop the
/// instructions `keep` rejects.
///
/// Deleted instructions stay in the arena as unreferenced entries. Compacting
/// the arena would invalidate every [`Value`] any caller is holding, and the
/// arena is not the thing under memory pressure.
fn rebuild(f: &mut Func, map: &HashMap<Value, Value>, dead: &HashSet<Value>) {
    let all: Vec<Value> = (0..f.insts.len() as Value).collect();
    for v in all {
        if !map.is_empty() {
            f.remap_operands(v, map);
        }
    }
    if !map.is_empty() {
        for b in 0..f.blocks.len() {
            let t = f.blocks[b].term.clone();
            f.blocks[b].term = match t {
                Term::CondBr(c, x, y) => Term::CondBr(*map.get(&c).unwrap_or(&c), x, y),
                Term::Ret(Some(v)) => Term::Ret(Some(*map.get(&v).unwrap_or(&v))),
                other => other,
            };
        }
    }
    if !dead.is_empty() {
        for b in 0..f.blocks.len() {
            f.blocks[b].insts.retain(|v| !dead.contains(v));
        }
    }
}

/// Resolve a chain of replacements, so `a -> b -> c` maps `a` to `c`.
fn settle(map: &mut HashMap<Value, Value>) {
    let keys: Vec<Value> = map.keys().copied().collect();
    for k in keys {
        let mut v = map[&k];
        let mut guard = 0;
        while let Some(&next) = map.get(&v) {
            if next == v || guard > 64 {
                break;
            }
            v = next;
            guard += 1;
        }
        map.insert(k, v);
    }
}

fn const_real(f: &Func, v: Value) -> Option<f64> {
    match f.op(v) {
        Op::ConstReal(c) => Some(*c),
        _ => None,
    }
}
fn const_int(f: &Func, v: Value) -> Option<i64> {
    match f.op(v) {
        Op::ConstInt(c) => Some(*c),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// constant folding and algebraic identities
// ---------------------------------------------------------------------------

/// Fold constants and apply the identities that AD would otherwise have to
/// differentiate.
///
/// The identities are the ones that are *exactly* true in IEEE arithmetic for
/// the values that reach them, with two deliberate exclusions: `x * 0.0` is not
/// folded to `0.0` because it is not `0.0` when `x` is a NaN or an infinity,
/// and `x - x` is not folded for the same reason. An optimiser that is willing
/// to be wrong about a NaN is willing to be wrong about a gradient.
pub fn fold(module: &mut Module, id: FuncId, stats: &mut OptStats) -> usize {
    let f = module.get_mut(id);
    let mut map: HashMap<Value, Value> = HashMap::new();
    let mut made: Vec<(BlockId, Op, Ty)> = Vec::new();
    let mut n = 0;

    for v in f.live_insts() {
        let block = f.block_of(v);
        let fold_to_real = |c: f64, made: &mut Vec<(BlockId, Op, Ty)>| {
            made.push((block, Op::ConstReal(c), Ty::Real));
        };
        match f.op(v).clone() {
            Op::Un(o, a) => {
                if let Some(x) = const_real(f, a) {
                    fold_to_real(o.eval(x), &mut made);
                    map.insert(v, u32::MAX - made.len() as u32 + 1);
                    n += 1;
                    continue;
                }
                // neg(neg(x)) = x, exactly, including on NaN and signed zero.
                if o == UnOp::Neg {
                    if let Op::Un(UnOp::Neg, inner) = *f.op(a) {
                        map.insert(v, inner);
                        n += 1;
                    }
                }
            }
            Op::Bin(o, a, b) => {
                match (const_real(f, a), const_real(f, b)) {
                    (Some(x), Some(y)) => {
                        fold_to_real(o.eval(x, y), &mut made);
                        map.insert(v, u32::MAX - made.len() as u32 + 1);
                        n += 1;
                        continue;
                    }
                    (Some(x), None) => match o {
                        // 0 + x is x; 1 * x is x. Both exact.
                        BinOp::Add if x == 0.0 && x.is_sign_positive() => {
                            map.insert(v, b);
                            n += 1;
                        }
                        BinOp::Mul if x == 1.0 => {
                            map.insert(v, b);
                            n += 1;
                        }
                        _ => {}
                    },
                    (None, Some(y)) => match o {
                        BinOp::Add | BinOp::Sub if y == 0.0 && y.is_sign_positive() => {
                            map.insert(v, a);
                            n += 1;
                        }
                        BinOp::Mul | BinOp::Div if y == 1.0 => {
                            map.insert(v, a);
                            n += 1;
                        }
                        BinOp::Pow if y == 1.0 => {
                            map.insert(v, a);
                            n += 1;
                        }
                        _ => {}
                    },
                    (None, None) => {}
                }
            }
            Op::Int(o, a, b) => {
                if let (Some(x), Some(y)) = (const_int(f, a), const_int(f, b)) {
                    made.push((block, Op::ConstInt(o.eval(x, y)), Ty::Int));
                    map.insert(v, u32::MAX - made.len() as u32 + 1);
                    n += 1;
                } else if let Some(y) = const_int(f, b) {
                    match o {
                        IntOp::Add | IntOp::Sub if y == 0 => {
                            map.insert(v, a);
                            n += 1;
                        }
                        IntOp::Mul if y == 1 => {
                            map.insert(v, a);
                            n += 1;
                        }
                        _ => {}
                    }
                }
            }
            Op::ICmp(p, a, b) => {
                if let (Some(x), Some(y)) = (const_int(f, a), const_int(f, b)) {
                    made.push((block, Op::ConstInt(i64::from(p.apply_int(x, y))), Ty::Int));
                    map.insert(v, u32::MAX - made.len() as u32 + 1);
                    n += 1;
                }
            }
            Op::FCmp(p, a, b) => {
                if let (Some(x), Some(y)) = (const_real(f, a), const_real(f, b)) {
                    made.push((block, Op::ConstInt(i64::from(p.apply(x, y))), Ty::Int));
                    map.insert(v, u32::MAX - made.len() as u32 + 1);
                    n += 1;
                }
            }
            Op::Select(c, a, b) => {
                if let Some(k) = const_int(f, c) {
                    map.insert(v, if k != 0 { a } else { b });
                    n += 1;
                } else if a == b {
                    map.insert(v, a);
                    n += 1;
                }
            }
            Op::Phi(inc) => {
                // A phi whose incoming values are all the same value is that
                // value. This is what makes mem2reg's output readable, and
                // what removes the phis AD introduces for zero seeds.
                let first = inc.first().map(|(_, x)| *x);
                if let Some(first) = first {
                    if inc.iter().all(|(_, x)| *x == first) && first != v {
                        map.insert(v, first);
                        n += 1;
                    }
                }
            }
            _ => {}
        }
    }

    // The new constants were referred to by placeholder while scanning; place
    // them for real now and fix the placeholders up.
    let mut placed = Vec::new();
    for (block, op, ty) in made {
        // Constants go in the entry block, where they dominate every use.
        let entry = f.entry;
        let _ = block;
        let nv = f.push_front(entry, op, ty);
        placed.push(nv);
    }
    for value in map.values_mut() {
        if *value > u32::MAX - 1_000_000 {
            let idx = (u32::MAX - *value) as usize;
            *value = placed[idx];
        }
    }

    settle(&mut map);
    rebuild(f, &map, &HashSet::new());
    stats.folded += n;
    n
}

// ---------------------------------------------------------------------------
// global value numbering
// ---------------------------------------------------------------------------

/// Key for "the same computation". Floats are hashed by bit pattern, which
/// keeps `0.0` and `-0.0` distinct -- they are distinct, and conflating them
/// changes the sign of a derivative through `1/x`.
#[derive(PartialEq, Eq, Hash)]
struct Key(u8, Vec<u64>);

fn key_of(f: &Func, v: Value) -> Option<Key> {
    let words = |xs: &[Value]| xs.iter().map(|x| u64::from(*x)).collect::<Vec<u64>>();
    Some(match f.op(v) {
        Op::ConstReal(c) => Key(0, vec![c.to_bits()]),
        Op::ConstInt(c) => Key(1, vec![*c as u64]),
        Op::Param(i) => Key(2, vec![u64::from(*i)]),
        Op::Un(o, a) => Key(3, vec![*o as u64, u64::from(*a)]),
        Op::Bin(o, a, b) => {
            let (mut x, mut y) = (u64::from(*a), u64::from(*b));
            if o.commutative() && x > y {
                std::mem::swap(&mut x, &mut y);
            }
            Key(4, vec![*o as u64, x, y])
        }
        Op::Int(o, a, b) => Key(5, vec![*o as u64, u64::from(*a), u64::from(*b)]),
        Op::FCmp(p, a, b) => Key(6, vec![*p as u64, u64::from(*a), u64::from(*b)]),
        Op::ICmp(p, a, b) => Key(7, vec![*p as u64, u64::from(*a), u64::from(*b)]),
        Op::Select(c, a, b) => Key(8, words(&[*c, *a, *b])),
        Op::Gep(p, o) => Key(9, words(&[*p, *o])),
        Op::IntToReal(a) => Key(10, words(&[*a])),
        Op::RealToInt(a) => Key(11, words(&[*a])),
        // Loads, stores, allocas, calls and phis are not pure, or not pure
        // enough to number without alias information this IR does not carry.
        _ => return None,
    })
}

/// Replace each repeated pure computation with its first occurrence, where
/// that occurrence dominates the use.
pub fn gvn(module: &mut Module, id: FuncId, stats: &mut OptStats) -> usize {
    let f = module.get_mut(id);
    let dt = DomTree::build(f);
    let mut table: HashMap<Key, Vec<Value>> = HashMap::new();
    let mut map: HashMap<Value, Value> = HashMap::new();
    let mut n = 0;
    for v in f.live_insts() {
        let Some(k) = key_of(f, v) else { continue };
        let here = f.block_of(v);
        let slot = table.entry(k).or_default();
        if let Some(&prev) = slot
            .iter()
            .find(|&&p| dt.dominates(f.block_of(p), here) && p != v)
        {
            map.insert(v, prev);
            n += 1;
        } else {
            slot.push(v);
        }
    }
    settle(&mut map);
    rebuild(f, &map, &HashSet::new());
    stats.cse += n;
    n
}

// ---------------------------------------------------------------------------
// mem2reg
// ---------------------------------------------------------------------------

/// Promote scalar allocas to SSA values.
///
/// Only allocas of one slot whose pointer is used *directly* by loads and
/// stores are eligible: the moment a pointer is `gep`ed, stored, or handed to
/// a call, it can alias and the promotion would be unsound. Arrays therefore
/// stay in memory, which is right -- shadow memory exists for them.
pub fn mem2reg(module: &mut Module, id: FuncId, stats: &mut OptStats) -> usize {
    let f = module.get_mut(id);

    // 1. eligibility
    let mut candidates: Vec<Value> = Vec::new();
    let mut disqualified: HashSet<Value> = HashSet::new();
    for v in f.live_insts() {
        if let Op::Alloca(n) = *f.op(v) {
            if const_int(f, n) == Some(1) {
                candidates.push(v);
            }
        }
    }
    if candidates.is_empty() {
        return 0;
    }
    let cand: HashSet<Value> = candidates.iter().copied().collect();
    for v in f.live_insts() {
        match f.op(v).clone() {
            Op::Load(p) => {
                let _ = p;
            }
            Op::Store(p, x) => {
                // Storing the pointer itself lets it escape.
                if cand.contains(&x) {
                    disqualified.insert(x);
                }
                let _ = p;
            }
            other => {
                for a in {
                    let mut tmp = Vec::new();
                    match &other {
                        Op::Gep(p, o) => {
                            tmp.push(*p);
                            tmp.push(*o);
                        }
                        Op::Call(_, args) => tmp.extend_from_slice(args),
                        Op::Select(c, a, b) => {
                            tmp.push(*c);
                            tmp.push(*a);
                            tmp.push(*b);
                        }
                        Op::Phi(inc) => tmp.extend(inc.iter().map(|(_, x)| *x)),
                        _ => {}
                    }
                    tmp
                } {
                    if cand.contains(&a) {
                        disqualified.insert(a);
                    }
                }
            }
        }
    }
    candidates.retain(|a| !disqualified.contains(a));
    if candidates.is_empty() {
        return 0;
    }

    let dt = DomTree::build(f);
    let df = dt.frontiers(f);
    let preds = f.preds();
    let mut promoted = 0;
    let mut dead: HashSet<Value> = HashSet::new();
    let mut map: HashMap<Value, Value> = HashMap::new();

    for &alloca in &candidates {
        // the type stored through it
        let mut ty = Ty::Real;
        let mut stores: Vec<Value> = Vec::new();
        let mut loads: Vec<Value> = Vec::new();
        for v in f.live_insts() {
            match *f.op(v) {
                Op::Store(p, x) if p == alloca => {
                    ty = f.ty(x);
                    stores.push(v);
                }
                Op::Load(p) if p == alloca => {
                    ty = f.ty(v);
                    loads.push(v);
                }
                _ => {}
            }
        }
        if stores.is_empty() {
            continue;
        }

        // 2. phi placement on the iterated dominance frontier
        let mut work: Vec<BlockId> = stores.iter().map(|&s| f.block_of(s)).collect();
        let mut has_phi: HashMap<BlockId, Value> = HashMap::new();
        let mut seen: HashSet<BlockId> = HashSet::new();
        while let Some(b) = work.pop() {
            for &d in &df[b as usize] {
                if has_phi.contains_key(&d) {
                    continue;
                }
                let phi = f.push_front(d, Op::Phi(Vec::new()), ty);
                has_phi.insert(d, phi);
                if seen.insert(d) {
                    work.push(d);
                }
            }
        }

        // 3. renaming, walking the dominator tree
        // Memory starts zeroed, so an uninitialised read really is zero, and
        // that is a fact about this IR rather than a convenient default.
        let zero = match ty {
            Ty::Real => f.push_front(f.entry, Op::ConstReal(0.0), Ty::Real),
            _ => f.push_front(f.entry, Op::ConstInt(0), Ty::Int),
        };
        let store_set: HashMap<Value, Value> = f
            .live_insts()
            .into_iter()
            .filter_map(|v| match *f.op(v) {
                Op::Store(p, x) if p == alloca => Some((v, x)),
                _ => None,
            })
            .collect();
        let load_set: HashSet<Value> = loads.iter().copied().collect();

        let mut stack: Vec<Value> = vec![zero];
        // (block, phase) -- phase 0 processes, phase 1 pops.
        let mut walk: Vec<(BlockId, u8, usize)> = vec![(f.entry, 0, 0)];
        let mut incoming: Vec<(Value, BlockId, Value)> = Vec::new();
        while let Some((b, phase, pushed)) = walk.pop() {
            if phase == 1 {
                for _ in 0..pushed {
                    stack.pop();
                }
                continue;
            }
            let mut pushed_here = 0usize;
            if let Some(&phi) = has_phi.get(&b) {
                stack.push(phi);
                pushed_here += 1;
            }
            for &v in &f.blocks[b as usize].insts.clone() {
                if let Some(&x) = store_set.get(&v) {
                    stack.push(x);
                    pushed_here += 1;
                    dead.insert(v);
                } else if load_set.contains(&v) {
                    map.insert(v, *stack.last().unwrap());
                    dead.insert(v);
                }
            }
            for s in f.succs(b) {
                if let Some(&phi) = has_phi.get(&s) {
                    incoming.push((phi, b, *stack.last().unwrap()));
                }
            }
            walk.push((b, 1, pushed_here));
            for &c in &dt.children[b as usize] {
                walk.push((c, 0, 0));
            }
        }
        for (phi, from, val) in incoming {
            if let Op::Phi(inc) = &mut f.insts[phi as usize].op {
                inc.push((from, val));
            }
        }
        // A phi in a block some of whose predecessors were never walked (an
        // unreachable pred) still needs an entry, or the verifier objects.
        for (&b, &phi) in &has_phi {
            let have: HashSet<BlockId> = match f.op(phi) {
                Op::Phi(inc) => inc.iter().map(|(b, _)| *b).collect(),
                _ => HashSet::new(),
            };
            let missing: Vec<BlockId> = preds[b as usize]
                .iter()
                .copied()
                .filter(|p| !have.contains(p))
                .collect();
            if let Op::Phi(inc) = &mut f.insts[phi as usize].op {
                for p in missing {
                    inc.push((p, zero));
                }
            }
        }
        dead.insert(alloca);
        promoted += 1;
    }

    settle(&mut map);
    rebuild(f, &map, &dead);
    stats.promoted_allocas += promoted;
    promoted
}

// ---------------------------------------------------------------------------
// loop-invariant code motion
// ---------------------------------------------------------------------------

/// Hoist pure loop-invariant instructions into the loop preheader.
///
/// A loop with no single dedicated preheader is skipped rather than given one.
/// Splitting an edge means rewriting every phi in the header, and getting that
/// subtly wrong would show up as a wrong gradient rather than a crash.
pub fn licm(module: &mut Module, id: FuncId, stats: &mut OptStats) -> usize {
    let f = module.get_mut(id);
    let dt = DomTree::build(f);
    let ls = loops(f, &dt);
    let preds = f.preds();
    let mut moved = 0;

    for l in &ls {
        let outside: Vec<BlockId> = preds[l.header as usize]
            .iter()
            .copied()
            .filter(|p| !l.body.contains(p))
            .collect();
        if outside.len() != 1 {
            continue;
        }
        let pre = outside[0];
        if f.succs(pre).len() != 1 {
            continue;
        }
        // A store anywhere in the loop makes every load in it non-invariant,
        // because this IR carries no alias information to say otherwise.
        let has_store = l.body.iter().any(|&b| {
            f.blocks[b as usize]
                .insts
                .iter()
                .any(|&v| matches!(f.op(v), Op::Store(_, _) | Op::Call(_, _)))
        });

        loop {
            let mut hoisted_this_round = false;
            for &b in &l.body.clone() {
                for &v in &f.blocks[b as usize].insts.clone() {
                    let op = f.op(v).clone();
                    let hoistable = match &op {
                        Op::Phi(_) | Op::Alloca(_) | Op::Store(_, _) | Op::Call(_, _) => false,
                        Op::Load(_) => !has_store,
                        _ => true,
                    };
                    if !hoistable {
                        continue;
                    }
                    let invariant = f
                        .operands(v)
                        .iter()
                        .all(|&o| !l.body.contains(&f.block_of(o)));
                    if !invariant {
                        continue;
                    }
                    // Move it: same value id, different block.
                    f.blocks[b as usize].insts.retain(|&x| x != v);
                    f.blocks[pre as usize].insts.push(v);
                    f.insts[v as usize].block = pre;
                    moved += 1;
                    hoisted_this_round = true;
                }
            }
            if !hoisted_this_round {
                break;
            }
        }
    }
    stats.hoisted += moved;
    moved
}

// ---------------------------------------------------------------------------
// dead code elimination
// ---------------------------------------------------------------------------

/// Delete instructions nothing observes, and blocks nothing reaches.
pub fn dce(module: &mut Module, id: FuncId, stats: &mut OptStats) -> usize {
    let f = module.get_mut(id);

    // unreachable blocks first, so their phi entries go too
    let reachable: HashSet<BlockId> = f.rpo().into_iter().collect();
    let mut removed_edges = 0;
    if reachable.len() != f.blocks.len() {
        for b in 0..f.blocks.len() {
            if !reachable.contains(&(b as BlockId)) {
                f.blocks[b].insts.clear();
                f.blocks[b].term = Term::Ret(None);
                removed_edges += 1;
            }
        }
        let live_insts = f.live_insts();
        for v in live_insts {
            if let Op::Phi(inc) = &mut f.insts[v as usize].op {
                inc.retain(|(b, _)| reachable.contains(b));
            }
        }
    }

    let mut live: HashSet<Value> = HashSet::new();
    let mut work: Vec<Value> = Vec::new();
    for b in f.rpo() {
        for &v in &f.blocks[b as usize].insts {
            if f.has_effect(v) {
                work.push(v);
            }
        }
        match &f.blocks[b as usize].term {
            Term::CondBr(c, _, _) => work.push(*c),
            Term::Ret(Some(v)) => work.push(*v),
            _ => {}
        }
    }
    while let Some(v) = work.pop() {
        if !live.insert(v) {
            continue;
        }
        for o in f.operands(v) {
            work.push(o);
        }
    }
    let mut dead: HashSet<Value> = HashSet::new();
    for b in f.rpo() {
        for &v in &f.blocks[b as usize].insts {
            if !live.contains(&v) {
                dead.insert(v);
            }
        }
    }
    let n = dead.len();
    rebuild(f, &HashMap::new(), &dead);
    stats.deleted += n;
    n + removed_edges
}
