//! Forward mode: an IR-to-IR transform carrying tangents alongside the primal.
//!
//! The generated function has the same control flow graph as the original --
//! same blocks, same branches, same phis -- with a tangent instruction beside
//! every active primal instruction and *nothing at all* beside the inactive
//! ones. That last part is [`crate::activity`] earning its place: on the
//! benchmark programs it is most of the instructions.
//!
//! # The boundary
//!
//! Parameters are expanded in place, following Enzyme's convention:
//!
//! | annotation | becomes |
//! | ---------- | ------- |
//! | `Const` | the parameter, alone |
//! | `Active` | the parameter, then its tangent |
//! | `Duplicated` / `DupNoNeed` | the pointer, then a shadow pointer of the same shape |
//!
//! and the function returns the tangent of the original return value. One call
//! computes one directional derivative; `n` calls with the `n` basis seeds give
//! the full Jacobian, which is why forward mode is the right choice exactly
//! when the input dimension is small.
//!
//! # Memory
//!
//! Every pointer has a shadow. An `alloca` gets a shadow `alloca`; a `gep` gets
//! the same `gep` on the shadow base; a load out of an active object becomes a
//! second load out of its shadow, and a store becomes a second store. The
//! shadow's addressing is identical to the primal's by construction, so no
//! aliasing question ever has to be answered.

use crate::activity::{analyse, Activity, ActivityInfo};
use crate::ir::*;
use crate::typeanalysis::{self, Prim, TypeInfo};
use std::collections::HashMap;

/// What one differentiation asked for, kept together so the transform, the
/// C ABI and the tests all describe a job the same way.
#[derive(Clone, Debug)]
pub struct Job {
    pub params: Vec<Activity>,
    /// Pointee types for pointer parameters, from the caller. Type analysis
    /// usually infers these; a buffer the function only writes needs the hint.
    pub hints: HashMap<u32, Prim>,
    /// How many slots the reverse pass may use for its tapes.
    ///
    /// Real Enzyme grows its tape as it goes. This one does not: the size is
    /// stated up front, because a fixed capacity is a number the caller can
    /// see and reason about, and a silently growing one is a memory profile
    /// nobody predicted. Forward mode ignores this -- it has no tape at all,
    /// which is the entire reason to prefer it when the inputs are few.
    pub tape_capacity: i64,
    /// Custom derivative rules: a callee, and the function in the same module
    /// that computes its derivative. Both take one real and return one.
    ///
    /// A call with a rule is differentiated *as a call*: the transforms emit
    /// `g(u)` and chain it, rather than looking inside `f`. A call without one
    /// is opaque, and reverse mode refuses it by name. The rule is a claim the
    /// caller made, and nothing here checks it -- that is the verifier's job,
    /// which runs the real `f` and has no reason to believe `g`.
    pub rules: HashMap<FuncId, FuncId>,
}

impl Job {
    pub fn new(params: Vec<Activity>) -> Self {
        Job {
            params,
            hints: HashMap::new(),
            tape_capacity: 1 << 16,
            rules: HashMap::new(),
        }
    }
    pub fn hint(mut self, param: u32, p: Prim) -> Self {
        self.hints.insert(param, p);
        self
    }
    /// Register `derivative` as the derivative of `callee`.
    pub fn rule(mut self, callee: FuncId, derivative: FuncId) -> Self {
        self.rules.insert(callee, derivative);
        self
    }
    pub fn tape(mut self, slots: i64) -> Self {
        self.tape_capacity = slots;
        self
    }
    /// Where each active scalar's gradient lands in the output buffer.
    pub fn active_slots(&self) -> Vec<u32> {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == Activity::Active)
            .map(|(i, _)| i as u32)
            .collect()
    }
}

/// Counts for the log: how much of the program needed a tangent at all.
#[derive(Clone, Debug, Default)]
pub struct FwdStats {
    pub primal_insts: usize,
    pub active_insts: usize,
    pub tangent_insts: usize,
    pub shadow_allocas: usize,
}

/// Differentiate `id` in forward mode, adding the new function to the module.
pub fn fwddiff(module: &mut Module, id: FuncId, job: &Job) -> (FuncId, FwdStats) {
    let types = typeanalysis::infer(module, id, &job.hints);
    let act = analyse(module, id, &job.params, true, &types);
    build(module, id, job, &types, &act)
}

fn build(
    module: &mut Module,
    id: FuncId,
    job: &Job,
    types: &TypeInfo,
    act: &ActivityInfo,
) -> (FuncId, FwdStats) {
    let src = module.get(id).clone();
    let mut stats = FwdStats {
        primal_insts: src.inst_count(),
        active_insts: act.count_active(),
        ..Default::default()
    };

    // parameter layout
    let mut params = Vec::new();
    // Interleaving tangents renumbers the parameters, so the primal cannot go
    // on reading `Param(i)`: the original parameter 1 is no longer at index 1.
    // Reading the seed instead of the argument is a wrong derivative when the
    // argument is a real, and an unbounded loop when it is a trip count, so
    // this map is not an optimisation.
    let mut param_map: HashMap<u32, u32> = HashMap::new();
    let mut tangent_param: HashMap<u32, u32> = HashMap::new();
    let mut shadow_param: HashMap<u32, u32> = HashMap::new();
    for (i, t) in src.params.iter().enumerate() {
        param_map.insert(i as u32, params.len() as u32);
        params.push(*t);
        match job.params.get(i).copied().unwrap_or(Activity::Const) {
            Activity::Active => {
                tangent_param.insert(i as u32, params.len() as u32);
                params.push(Ty::Real);
            }
            Activity::Duplicated | Activity::DupNoNeed => {
                shadow_param.insert(i as u32, params.len() as u32);
                params.push(Ty::Ptr);
            }
            Activity::Const => {}
        }
    }

    let mut out = Func::new(&format!("{}__fwd", src.name), params, Ty::Real);
    // Mirror the CFG exactly. Block 0 already exists as the entry.
    for b in 1..src.blocks.len() {
        out.new_block(&src.blocks[b].label);
    }
    for b in 0..src.blocks.len() {
        out.blocks[b].label = format!("{}_", src.blocks[b].label);
    }

    let zero = out.push(0, Op::ConstReal(0.0), Ty::Real);

    let mut pmap: HashMap<Value, Value> = HashMap::new();
    let mut tmap: HashMap<Value, Value> = HashMap::new();
    let mut shadow: HashMap<Value, Value> = HashMap::new();
    // phis whose incoming lists have to be stitched once everything exists
    let mut phi_fix: Vec<(Value, Value, bool)> = Vec::new(); // (new, old, is_tangent)

    for b in src.rpo() {
        for &v in &src.blocks[b as usize].insts {
            let op = src.op(v).clone();
            let ty = src.ty(v);
            let m = |x: Value, pmap: &HashMap<Value, Value>| *pmap.get(&x).unwrap_or(&x);

            // ---- primal ----
            let np = match &op {
                Op::Phi(_) => {
                    let p = out.push_front(b, Op::Phi(Vec::new()), ty);
                    phi_fix.push((p, v, false));
                    p
                }
                Op::Param(i) => {
                    let at = *param_map.get(i).unwrap_or(i);
                    out.push(b, Op::Param(at), ty)
                }
                other => {
                    let mut cloned = other.clone();
                    let tmp = out.push(b, cloned.clone(), ty);
                    cloned = out.insts[tmp as usize].op.clone();
                    let _ = cloned;
                    out.remap_operands(tmp, &pmap);
                    tmp
                }
            };
            pmap.insert(v, np);
            let _ = m;

            // ---- shadow pointers ----
            if ty == Ty::Ptr || matches!(op, Op::Alloca(_)) {
                let sh = match &op {
                    Op::Param(i) => shadow_param
                        .get(i)
                        .map(|si| out.push(b, Op::Param(*si), Ty::Ptr)),
                    Op::Alloca(n) => {
                        stats.shadow_allocas += 1;
                        let nn = *pmap.get(n).unwrap_or(n);
                        Some(out.push(b, Op::Alloca(nn), Ty::Ptr))
                    }
                    Op::Gep(p, o) => shadow.get(p).map(|sp| {
                        let no = *pmap.get(o).unwrap_or(o);
                        out.push(b, Op::Gep(*sp, no), Ty::Ptr)
                    }),
                    Op::Phi(_) => {
                        let p = out.push_front(b, Op::Phi(Vec::new()), Ty::Ptr);
                        phi_fix.push((p, v, true));
                        Some(p)
                    }
                    Op::Select(c, x, y) => match (shadow.get(x), shadow.get(y)) {
                        (Some(sx), Some(sy)) => {
                            let nc = *pmap.get(c).unwrap_or(c);
                            Some(out.push(b, Op::Select(nc, *sx, *sy), Ty::Ptr))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(sh) = sh {
                    shadow.insert(v, sh);
                }
            }

            // ---- tangent ----
            if !act.is_active(v) && !matches!(op, Op::Store(_, _)) {
                continue;
            }
            let t = |x: Value, tmap: &HashMap<Value, Value>| *tmap.get(&x).unwrap_or(&zero);
            let p = |x: Value, pmap: &HashMap<Value, Value>| *pmap.get(&x).unwrap_or(&x);
            let before = out.insts.len();
            let tangent: Option<Value> = match &op {
                Op::ConstReal(_) | Op::ConstInt(_) => None,
                Op::Param(i) => tangent_param
                    .get(i)
                    .map(|ti| out.push(b, Op::Param(*ti), Ty::Real)),
                Op::Un(o, a) => {
                    let ta = t(*a, &tmap);
                    let pa = p(*a, &pmap);
                    Some(unary_tangent(&mut out, b, *o, pa, np, ta))
                }
                Op::Bin(o, a, bb) => {
                    let (ta, tb) = (t(*a, &tmap), t(*bb, &tmap));
                    let (pa, pb) = (p(*a, &pmap), p(*bb, &pmap));
                    Some(binary_tangent(&mut out, b, *o, pa, pb, np, ta, tb, zero))
                }
                Op::Select(c, x, y) => {
                    let nc = p(*c, &pmap);
                    let (tx, ty2) = (t(*x, &tmap), t(*y, &tmap));
                    Some(out.push(b, Op::Select(nc, tx, ty2), Ty::Real))
                }
                Op::Phi(_) => {
                    let ph = out.push_front(b, Op::Phi(Vec::new()), Ty::Real);
                    phi_fix.push((ph, v, true));
                    Some(ph)
                }
                Op::Load(ptr) => shadow
                    .get(ptr)
                    .map(|sp| out.push(b, Op::Load(*sp), Ty::Real)),
                Op::Store(ptr, x) => {
                    if let Some(sp) = shadow.get(ptr).copied() {
                        // Only store a tangent where the object carries one.
                        if types.behind(*ptr) == Prim::Real {
                            let tx = t(*x, &tmap);
                            out.push(b, Op::Store(sp, tx), Ty::Void);
                        }
                    }
                    None
                }
                Op::IntToReal(_) | Op::RealToInt(_) | Op::FCmp(_, _, _) | Op::ICmp(_, _, _) => None,
                Op::Int(_, _, _) | Op::Alloca(_) | Op::Gep(_, _) => None,
                // A call with a registered rule: y = f(u) has tangent
                // g(u) * u', with g emitted as a call of its own. Without a
                // rule the call is opaque here, exactly as it is in reverse
                // mode; the front ends refuse such a call before this point.
                Op::Call(callee, args) => match (job.rules.get(callee), args.first()) {
                    (Some(&g), Some(&u)) if args.len() == 1 => {
                        let pu = p(u, &pmap);
                        let tu = t(u, &tmap);
                        let d = out.push(b, Op::Call(g, vec![pu]), Ty::Real);
                        Some(out.push(b, Op::Bin(BinOp::Mul, d, tu), Ty::Real))
                    }
                    _ => None,
                },
            };
            stats.tangent_insts += out.insts.len() - before;
            if let Some(tv) = tangent {
                tmap.insert(v, tv);
            }
        }

        // terminator
        out.blocks[b as usize].term = match &src.blocks[b as usize].term {
            Term::Br(x) => Term::Br(*x),
            Term::CondBr(c, x, y) => Term::CondBr(*pmap.get(c).unwrap_or(c), *x, *y),
            Term::Ret(Some(v)) => Term::Ret(Some(*tmap.get(v).unwrap_or(&zero))),
            Term::Ret(None) => Term::Ret(None),
            Term::Unset => Term::Unset,
        };
    }

    // stitch the phis, now that every value exists
    for (newphi, oldphi, is_shadow_or_tangent) in phi_fix {
        let Op::Phi(inc) = src.op(oldphi).clone() else {
            continue;
        };
        let mut built = Vec::new();
        for (fromb, x) in inc {
            let val = if is_shadow_or_tangent {
                if out.ty(newphi) == Ty::Ptr {
                    *shadow.get(&x).unwrap_or(&zero)
                } else {
                    *tmap.get(&x).unwrap_or(&zero)
                }
            } else {
                *pmap.get(&x).unwrap_or(&x)
            };
            built.push((fromb, val));
        }
        out.insts[newphi as usize].op = Op::Phi(built);
    }

    let new_id = module.add(out);
    (new_id, stats)
}

/// `d/dx op(x) * tangent`, reusing the primal result where the derivative is a
/// function of it -- `exp`, `sqrt` and `tanh` all are, and recomputing them
/// would double the transcendental count for nothing.
fn unary_tangent(
    out: &mut Func,
    b: BlockId,
    o: UnOp,
    pa: Value,
    primal: Value,
    ta: Value,
) -> Value {
    let r = |out: &mut Func, op: Op| out.push(b, op, Ty::Real);
    let mul = |out: &mut Func, x: Value, y: Value| out.push(b, Op::Bin(BinOp::Mul, x, y), Ty::Real);
    match o {
        UnOp::Neg => r(out, Op::Un(UnOp::Neg, ta)),
        UnOp::Sin => {
            let c = r(out, Op::Un(UnOp::Cos, pa));
            mul(out, c, ta)
        }
        UnOp::Cos => {
            let s = r(out, Op::Un(UnOp::Sin, pa));
            let n = r(out, Op::Un(UnOp::Neg, s));
            mul(out, n, ta)
        }
        UnOp::Tan => {
            // sec^2 = 1 + tan^2, and tan is the primal we already have.
            let sq = mul(out, primal, primal);
            let one = r(out, Op::ConstReal(1.0));
            let s = r(out, Op::Bin(BinOp::Add, one, sq));
            mul(out, s, ta)
        }
        UnOp::Exp => mul(out, primal, ta),
        UnOp::Log => r(out, Op::Bin(BinOp::Div, ta, pa)),
        UnOp::Sqrt => {
            let two = r(out, Op::ConstReal(2.0));
            let d = mul(out, two, primal);
            r(out, Op::Bin(BinOp::Div, ta, d))
        }
        UnOp::Tanh => {
            let sq = mul(out, primal, primal);
            let one = r(out, Op::ConstReal(1.0));
            let s = r(out, Op::Bin(BinOp::Sub, one, sq));
            mul(out, s, ta)
        }
        UnOp::Sinh => {
            let c = r(out, Op::Un(UnOp::Cosh, pa));
            mul(out, c, ta)
        }
        UnOp::Cosh => {
            let s = r(out, Op::Un(UnOp::Sinh, pa));
            mul(out, s, ta)
        }
        UnOp::Abs => {
            // Not differentiable at zero. The convention taken is the one every
            // AD system takes -- the right derivative -- and it is written down
            // here rather than left to be discovered.
            let z = r(out, Op::ConstReal(0.0));
            let ge = out.push(b, Op::FCmp(Pred::Ge, pa, z), Ty::Int);
            let neg = r(out, Op::Un(UnOp::Neg, ta));
            r(out, Op::Select(ge, ta, neg))
        }
        UnOp::Recip => {
            let sq = mul(out, primal, primal);
            let m = mul(out, sq, ta);
            r(out, Op::Un(UnOp::Neg, m))
        }
        UnOp::Erf => {
            let sq = mul(out, pa, pa);
            let n = r(out, Op::Un(UnOp::Neg, sq));
            let e = r(out, Op::Un(UnOp::Exp, n));
            let c = r(out, Op::ConstReal(std::f64::consts::FRAC_2_SQRT_PI));
            let k = mul(out, c, e);
            mul(out, k, ta)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn binary_tangent(
    out: &mut Func,
    b: BlockId,
    o: BinOp,
    pa: Value,
    pb: Value,
    primal: Value,
    ta: Value,
    tb: Value,
    zero: Value,
) -> Value {
    let r = |out: &mut Func, op: Op| out.push(b, op, Ty::Real);
    let mul = |out: &mut Func, x: Value, y: Value| out.push(b, Op::Bin(BinOp::Mul, x, y), Ty::Real);
    match o {
        BinOp::Add => r(out, Op::Bin(BinOp::Add, ta, tb)),
        BinOp::Sub => r(out, Op::Bin(BinOp::Sub, ta, tb)),
        BinOp::Mul => {
            let l = mul(out, ta, pb);
            let rr = mul(out, pa, tb);
            r(out, Op::Bin(BinOp::Add, l, rr))
        }
        BinOp::Div => {
            // (ta - (a/b) * tb) / b, using the primal quotient we already have.
            let q = mul(out, primal, tb);
            let n = r(out, Op::Bin(BinOp::Sub, ta, q));
            r(out, Op::Bin(BinOp::Div, n, pb))
        }
        BinOp::Pow => {
            // d(a^b) = b * a^(b-1) * ta + a^b * log a * tb. The first term is
            // written as its own power rather than as a^b * b / a, which is
            // 0/0 at a = 0 and made x^2 undifferentiable at the origin. When
            // the exponent is a constant -- overwhelmingly the common case --
            // tb is the shared zero and the log term is skipped; otherwise
            // log a is guarded at a <= 0, where a^b * log a has limit 0 for
            // b > 0 and no derivative exists for b <= 0 anyway.
            let one = r(out, Op::ConstReal(1.0));
            let bm1 = r(out, Op::Bin(BinOp::Sub, pb, one));
            let apow = r(out, Op::Bin(BinOp::Pow, pa, bm1));
            let da = mul(out, pb, apow);
            let term1 = mul(out, da, ta);
            if tb == zero {
                term1
            } else {
                let z = r(out, Op::ConstReal(0.0));
                let positive = out.push(b, Op::FCmp(Pred::Gt, pa, z), Ty::Int);
                let lg = r(out, Op::Un(UnOp::Log, pa));
                let lg = r(out, Op::Select(positive, lg, z));
                let db = mul(out, primal, lg);
                let term2 = mul(out, db, tb);
                r(out, Op::Bin(BinOp::Add, term1, term2))
            }
        }
        BinOp::Max => {
            let ge = out.push(b, Op::FCmp(Pred::Ge, pa, pb), Ty::Int);
            r(out, Op::Select(ge, ta, tb))
        }
        BinOp::Min => {
            let le = out.push(b, Op::FCmp(Pred::Le, pa, pb), Ty::Int);
            r(out, Op::Select(le, ta, tb))
        }
        BinOp::Atan2 => {
            let n1 = mul(out, pb, ta);
            let n2 = mul(out, pa, tb);
            let num = r(out, Op::Bin(BinOp::Sub, n1, n2));
            let a2 = mul(out, pa, pa);
            let b2 = mul(out, pb, pb);
            let den = r(out, Op::Bin(BinOp::Add, a2, b2));
            r(out, Op::Bin(BinOp::Div, num, den))
        }
    }
}
