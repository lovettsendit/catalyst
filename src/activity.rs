//! Activity analysis: deciding what does not need a derivative.
//!
//! This is the single largest optimisation in an AD compiler, and the reason
//! Enzyme's output is competitive with hand-written adjoints. In a real kernel
//! most instructions are loop counters, addresses and predicates. None of them
//! can carry a derivative, and generating adjoint code for them would roughly
//! double the program for no result.
//!
//! A value is **active** when both of these hold:
//!
//! * it is **varied** -- some differentiated input can change it, and
//! * it is **useful** -- it can change the differentiated output.
//!
//! Neither half is sufficient. A loop counter is useful (it decides the answer)
//! but not varied. A debug statistic accumulated from the inputs is varied but
//! not useful. Only the intersection needs adjoint code, and only reals can be
//! in it at all.
//!
//! # Memory
//!
//! Activity is not only a property of registers. A memory object is active if
//! an active value is ever stored into it, and a load out of an active object
//! is varied. That fixpoint is what decides whether a buffer needs a shadow --
//! and shadows are the expensive thing, so getting this right is most of the
//! difference between a gradient that fits in cache and one that does not.

use crate::ir::*;
use crate::typeanalysis::{Prim, TypeInfo};
use std::collections::HashSet;

/// What the caller wants differentiated, per parameter. The same vocabulary
/// Enzyme puts at its boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Activity {
    /// Not differentiated. No shadow, no adjoint.
    Const,
    /// A scalar whose derivative is wanted back.
    Active,
    /// A pointer; the caller supplies a shadow buffer of the same shape and
    /// gradients accumulate into it.
    Duplicated,
    /// As `Duplicated`, but the primal result is not needed, so the primal
    /// computation may be dead-code eliminated where nothing else reads it.
    DupNoNeed,
}

impl Activity {
    pub fn is_const(self) -> bool {
        self == Activity::Const
    }
    pub fn shadowed(self) -> bool {
        matches!(self, Activity::Duplicated | Activity::DupNoNeed)
    }
}

/// The result: which values and which memory objects are active.
pub struct ActivityInfo {
    pub varied: Vec<bool>,
    pub useful: Vec<bool>,
    pub active: Vec<bool>,
    pub active_object: Vec<bool>,
}

impl ActivityInfo {
    pub fn is_active(&self, v: Value) -> bool {
        self.active[v as usize]
    }
    pub fn count_active(&self) -> usize {
        self.active.iter().filter(|x| **x).count()
    }
}

/// Run the analysis.
///
/// `params` gives the caller's annotation per formal parameter; `ret_active`
/// says whether the returned value is the thing being differentiated.
pub fn analyse(
    module: &Module,
    id: FuncId,
    params: &[Activity],
    ret_active: bool,
    types: &TypeInfo,
) -> ActivityInfo {
    let f = module.get(id);
    let n = f.insts.len();
    let nobj = types.objects();

    let mut varied = vec![false; n];
    let mut active_object = vec![false; nobj];

    // Seed: parameters the caller asked for.
    for v in f.live_insts() {
        if let Op::Param(i) = *f.op(v) {
            match params.get(i as usize).copied().unwrap_or(Activity::Const) {
                Activity::Const => {}
                Activity::Active => varied[v as usize] = true,
                Activity::Duplicated | Activity::DupNoNeed => {
                    // The pointer itself is not varied -- the address does not
                    // move. Its *contents* are, which is a fact about the
                    // object rather than the register.
                    let o = types.object[v as usize];
                    if o != u32::MAX {
                        active_object[o as usize] = true;
                    }
                }
            }
        }
    }

    // Forward fixpoint: variation flows along data edges, and through memory.
    let order = f.live_insts();
    for _ in 0..64 {
        let mut changed = false;
        for &v in &order {
            if f.ty(v) == Ty::Int && !matches!(f.op(v), Op::Load(_)) {
                // Integer-typed results are structurally inactive. This is the
                // cheap half of the analysis and it removes most of the
                // program: indices, counters, predicates.
                continue;
            }
            let mut is = varied[v as usize];
            match f.op(v).clone() {
                Op::Load(p) => {
                    let o = types.object[p as usize];
                    if o != u32::MAX
                        && active_object[o as usize]
                        && types.behind(p).differentiable()
                    {
                        is = true;
                    }
                }
                Op::Store(p, x) => {
                    if varied[x as usize] {
                        let o = types.object[p as usize];
                        if o != u32::MAX && !active_object[o as usize] {
                            active_object[o as usize] = true;
                            changed = true;
                        }
                    }
                }
                Op::Alloca(_) | Op::Gep(_, _) | Op::ConstReal(_) | Op::ConstInt(_) => {}
                Op::Param(_) => {}
                Op::Select(_, a, b) => is |= varied[a as usize] || varied[b as usize],
                Op::FCmp(_, _, _) | Op::ICmp(_, _, _) | Op::RealToInt(_) | Op::IntToReal(_) => {
                    // A comparison of two varied reals is still a discrete
                    // choice: it has no derivative and does not propagate one.
                }
                _ => {
                    for o in f.operands(v) {
                        if varied[o as usize] {
                            is = true;
                        }
                    }
                }
            }
            if is && !varied[v as usize] {
                varied[v as usize] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Backward fixpoint: usefulness flows from the differentiated output.
    let mut useful = vec![false; n];
    if ret_active {
        for b in f.rpo() {
            if let Term::Ret(Some(v)) = f.blocks[b as usize].term {
                useful[v as usize] = true;
            }
        }
    }
    // A duplicated buffer is an output too: gradients accumulate into its
    // shadow, so anything stored into it is useful.
    let dup_objects: HashSet<u32> = f
        .live_insts()
        .into_iter()
        .filter_map(|v| match *f.op(v) {
            Op::Param(i)
                if params
                    .get(i as usize)
                    .copied()
                    .unwrap_or(Activity::Const)
                    .shadowed() =>
            {
                Some(types.object[v as usize])
            }
            _ => None,
        })
        .filter(|o| *o != u32::MAX)
        .collect();

    for _ in 0..64 {
        let mut changed = false;
        for &v in order.iter().rev() {
            let mut want = useful[v as usize];
            if let Op::Store(p, _) = *f.op(v) {
                let o = types.object[p as usize];
                if o != u32::MAX && (dup_objects.contains(&o) || active_object[o as usize]) {
                    want = true;
                }
            }
            if !want {
                continue;
            }
            for o in f.operands(v) {
                if !useful[o as usize] {
                    useful[o as usize] = true;
                    changed = true;
                }
            }
            if !useful[v as usize] {
                useful[v as usize] = true;
                changed = true;
            }
            // Reading an active object makes every store into it useful,
            // because the value read may be one of them.
            if let Op::Load(p) = *f.op(v) {
                let o = types.object[p as usize];
                if o != u32::MAX {
                    for &w in &order {
                        if let Op::Store(q, x) = *f.op(w) {
                            if types.object[q as usize] == o && !useful[x as usize] {
                                useful[x as usize] = true;
                                useful[w as usize] = true;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut active = vec![false; n];
    for &v in &order {
        let real = f.ty(v) == Ty::Real
            || (matches!(f.op(v), Op::Load(_))
                && types.load_ty.get(&v).copied().unwrap_or(Prim::Unknown) == Prim::Real);
        active[v as usize] = real && varied[v as usize] && useful[v as usize];
    }

    ActivityInfo {
        varied,
        useful,
        active,
        active_object,
    }
}
