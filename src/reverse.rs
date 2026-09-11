//! Reverse mode: adjoints accumulated backwards through the program.
//!
//! Forward mode costs one pass per input. Reverse mode costs one pass per
//! *output*, which is why every trained model in existence uses it: a loss is
//! one number and its parameters are millions, so one reverse pass buys the
//! whole gradient. That asymmetry is the entire reason this transform exists
//! beside [`crate::forward`].
//!
//! # What this build differentiates, and what it refuses
//!
//! Straight-line functions: one block, no branches, no loops. Every real
//! operation the IR has, `Select`, and memory through shadow pointers.
//!
//! Anything with control flow is **refused by name**, with
//! [`crate::refuse::Cause::BranchingControlFlow`], and the refusal points at
//! forward mode. It is not approximated and there is no flag that makes it try:
//! walking a program backwards requires knowing which path ran, that knowledge
//! comes from a recorded trace, and this build records none. Producing a
//! gradient anyway would mean assuming a path -- right sometimes, wrong
//! silently the rest of the time, and a silently wrong gradient is the one
//! defect this project refuses to ship. Forward mode has no such difficulty,
//! because it walks forwards alongside the primal and never has to ask.
//!
//! # Why the adjoints are SSA values and not a memory buffer
//!
//! In straight-line code every adjoint's definition dominates every use of it,
//! so the accumulation is just `Add` instructions and the optimiser can see
//! straight through them. A buffer would hide the same arithmetic behind loads
//! and stores that alias analysis then has to undo. Memory is used for exactly
//! the thing that needs it -- shadows of the program's own pointers.
//!
//! # The boundary
//!
//! Parameters are expanded the way [`crate::forward`] expands them, and then
//! two are appended:
//!
//! | parameter | meaning |
//! | --------- | ------- |
//! | `Const` | the parameter, alone |
//! | `Active` | the parameter, alone; its gradient is written to the output buffer |
//! | `Duplicated` / `DupNoNeed` | the pointer, then a shadow the gradient accumulates into |
//! | *(appended)* `gradients` | buffer written at each active parameter's own index |
//! | *(appended)* `seed` | the adjoint of the returned value, normally `1.0` |
//!
//! and the function **returns the primal result**. A training step wants the
//! loss and its gradient from one call, and returning the loss costs nothing
//! because the primal was computed anyway.
//!
//! [`crate::forward::Job::active_slots`] gives the caller the index map into
//! `gradients`, so nothing has to be recomputed on the other side of the ABI.

use crate::activity::{analyse, Activity, ActivityInfo};
use crate::forward::Job;
use crate::ir::*;
use crate::refuse::{Cause, Refusal};
use crate::typeanalysis::{self, Prim, TypeInfo};
use std::collections::HashMap;

/// Counts for the log: what the reverse pass had to build.
#[derive(Clone, Debug, Default)]
pub struct RevStats {
    pub primal_insts: usize,
    pub active_insts: usize,
    /// Instructions the reverse sweep added. Compare against `active_insts`:
    /// this is the real cost of a gradient, and it is the number to watch when
    /// a rule is changed.
    pub adjoint_insts: usize,
    pub shadow_allocas: usize,
}

/// Differentiate `id` in reverse mode, adding the new function to the module.
pub fn revdiff(module: &mut Module, id: FuncId, job: &Job) -> Result<(FuncId, RevStats), Refusal> {
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
) -> Result<(FuncId, RevStats), Refusal> {
    let src = module.get(id).clone();

    // ---- what this build will not pretend to do -------------------------
    if src.blocks.len() != 1 {
        return Err(Refusal::about_job(
            &src.name,
            Cause::BranchingControlFlow {
                blocks: src.blocks.len(),
            },
        ));
    }
    if job.params.iter().all(|a| matches!(a, Activity::Const)) {
        return Err(Refusal::about_job(&src.name, Cause::NothingActive));
    }
    // An annotation that disagrees with the type is caught before any code is
    // generated, because the generated code would be wrong in a way that looks
    // like a modelling mistake rather than a declaration one.
    for (i, annotation) in job.params.iter().enumerate() {
        let Some(ty) = src.params.get(i) else {
            continue;
        };
        let bad = match annotation {
            Activity::Active => *ty != Ty::Real,
            Activity::Duplicated | Activity::DupNoNeed => *ty != Ty::Ptr,
            Activity::Const => false,
        };
        if bad {
            return Err(Refusal::about_job(
                &src.name,
                Cause::AnnotationMismatch {
                    param: i as u32,
                    asked: format!("{annotation:?}"),
                    is: format!("{ty:?}"),
                },
            ));
        }
    }

    let mut stats = RevStats {
        primal_insts: src.inst_count(),
        active_insts: act.count_active(),
        ..Default::default()
    };

    // ---- parameter layout ------------------------------------------------
    let mut params = Vec::new();
    let mut param_map: HashMap<u32, u32> = HashMap::new();
    let mut shadow_param: HashMap<u32, u32> = HashMap::new();
    let mut active_params: Vec<u32> = Vec::new();
    for (i, t) in src.params.iter().enumerate() {
        param_map.insert(i as u32, params.len() as u32);
        params.push(*t);
        match job.params.get(i).copied().unwrap_or(Activity::Const) {
            Activity::Active => active_params.push(i as u32),
            Activity::Duplicated | Activity::DupNoNeed => {
                shadow_param.insert(i as u32, params.len() as u32);
                params.push(Ty::Ptr);
            }
            Activity::Const => {}
        }
    }
    let grad_param = params.len() as u32;
    params.push(Ty::Ptr);
    let seed_param = params.len() as u32;
    params.push(Ty::Real);

    let mut out = Func::new(&format!("{}__rev", src.name), params, Ty::Real);
    out.blocks[0].label = format!("{}_", src.blocks[0].label);
    let b: BlockId = 0;

    let zero = out.push(b, Op::ConstReal(0.0), Ty::Real);

    // ---- the primal, cloned ---------------------------------------------
    let mut pmap: HashMap<Value, Value> = HashMap::new();
    let mut shadow: HashMap<Value, Value> = HashMap::new();
    for &v in &src.blocks[0].insts {
        let op = src.op(v).clone();
        let ty = src.ty(v);
        if let Op::Call(callee, _) = &op {
            // Only refused when it can carry a derivative and no rule says
            // what that derivative is. A call whose result nothing
            // differentiable reads is just code, and stopping on it would
            // refuse programs this build handles correctly.
            if act.is_active(v) && !job.rules.contains_key(callee) {
                return Err(Refusal::at(
                    &src.name,
                    b,
                    &src.blocks[0].label,
                    v,
                    Cause::OpaqueCall {
                        callee: module.get(*callee).name.clone(),
                    },
                ));
            }
        }
        let np = match &op {
            Op::Param(i) => {
                let at = *param_map.get(i).unwrap_or(i);
                out.push(b, Op::Param(at), ty)
            }
            other => {
                let tmp = out.push(b, other.clone(), ty);
                out.remap_operands(tmp, &pmap);
                tmp
            }
        };
        pmap.insert(v, np);

        // Shadow pointers, allocated alongside the primal's own.
        if ty == Ty::Ptr || matches!(op, Op::Alloca(_)) {
            let sh = match &op {
                Op::Param(i) => shadow_param
                    .get(i)
                    .map(|si| out.push(b, Op::Param(*si), Ty::Ptr)),
                Op::Alloca(n) => {
                    stats.shadow_allocas += 1;
                    let nn = *pmap.get(n).unwrap_or(n);
                    // Memory::alloc zeroes, so a shadow starts at zero
                    // gradient without a clearing loop -- which is just as
                    // well, since a loop is the one thing this build refuses.
                    Some(out.push(b, Op::Alloca(nn), Ty::Ptr))
                }
                Op::Gep(p, o) => shadow.get(p).map(|sp| {
                    let no = *pmap.get(o).unwrap_or(o);
                    out.push(b, Op::Gep(*sp, no), Ty::Ptr)
                }),
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
    }

    let returned = match src.blocks[0].term {
        Term::Ret(Some(v)) => Some(v),
        _ => None,
    };

    // ---- the reverse sweep ----------------------------------------------
    let before_sweep = out.insts.len();
    let mut adj: HashMap<Value, Value> = HashMap::new();
    if let Some(r) = returned {
        let seed = out.push(b, Op::Param(seed_param), Ty::Real);
        adj.insert(r, seed);
    }

    for &v in src.blocks[0].insts.iter().rev() {
        let op = src.op(v).clone();
        // A store moves an adjoint out of memory even when the stored value is
        // itself inactive, so it is walked regardless. Everything else with no
        // adjoint has nothing to give away.
        let is_store = matches!(op, Op::Store(_, _));
        let Some(g) = adj
            .get(&v)
            .copied()
            .or(if is_store { Some(zero) } else { None })
        else {
            continue;
        };
        let p = |x: Value, pmap: &HashMap<Value, Value>| *pmap.get(&x).unwrap_or(&x);

        match &op {
            Op::Un(o, a) => {
                let pa = p(*a, &pmap);
                let primal = pmap[&v];
                let c = unary_adjoint(&mut out, b, *o, pa, primal, g);
                accumulate(&mut out, b, &mut adj, *a, c);
            }
            Op::Bin(o, a, bb) => {
                let (pa, pb) = (p(*a, &pmap), p(*bb, &pmap));
                let primal = pmap[&v];
                let (ca, cb) = binary_adjoint(&mut out, b, *o, pa, pb, primal, g);
                accumulate(&mut out, b, &mut adj, *a, ca);
                accumulate(&mut out, b, &mut adj, *bb, cb);
            }
            Op::Select(c, x, y) => {
                let nc = p(*c, &pmap);
                let gx = out.push(b, Op::Select(nc, g, zero), Ty::Real);
                let gy = out.push(b, Op::Select(nc, zero, g), Ty::Real);
                accumulate(&mut out, b, &mut adj, *x, gx);
                accumulate(&mut out, b, &mut adj, *y, gy);
            }
            Op::Load(ptr) => {
                // The gradient of a load lands in the shadow of the object it
                // read from, accumulating with whatever is already there.
                if let Some(sp) = shadow.get(ptr).copied() {
                    if types.behind(*ptr) == Prim::Real {
                        let had = out.push(b, Op::Load(sp), Ty::Real);
                        let sum = out.push(b, Op::Bin(BinOp::Add, had, g), Ty::Real);
                        out.push(b, Op::Store(sp, sum), Ty::Void);
                    }
                }
            }
            Op::Store(ptr, x) => {
                // A store overwrites, so its adjoint is taken and then cleared:
                // gradient that flowed into this location belongs to the value
                // stored here, and not to whatever was written before it.
                if let Some(sp) = shadow.get(ptr).copied() {
                    if types.behind(*ptr) == Prim::Real {
                        let had = out.push(b, Op::Load(sp), Ty::Real);
                        out.push(b, Op::Store(sp, zero), Ty::Void);
                        accumulate(&mut out, b, &mut adj, *x, had);
                    }
                }
            }
            // Leaves, and everything integral. `RealToInt` is a step function:
            // its derivative is zero wherever it is defined at all, so nothing
            // flows back through it, and saying so here is a rule rather than
            // an omission.
            Op::ConstReal(_)
            | Op::ConstInt(_)
            | Op::Param(_)
            | Op::Int(_, _, _)
            | Op::FCmp(_, _, _)
            | Op::ICmp(_, _, _)
            | Op::Alloca(_)
            | Op::Gep(_, _)
            | Op::IntToReal(_)
            | Op::RealToInt(_) => {}
            // A call with a registered rule: y = f(u), so u's adjoint gains
            // g * g_rule(u), with the rule's function called rather than
            // inlined. A call without a rule was refused above if it was
            // active, and has nothing to give away if it was not.
            Op::Call(callee, args) => {
                if let (Some(&rule), Some(&u)) = (job.rules.get(callee), args.first()) {
                    if args.len() == 1 {
                        let pu = p(u, &pmap);
                        let d = out.push(b, Op::Call(rule, vec![pu]), Ty::Real);
                        let c = out.push(b, Op::Bin(BinOp::Mul, g, d), Ty::Real);
                        accumulate(&mut out, b, &mut adj, u, c);
                    }
                }
            }
            Op::Phi(_) => {
                return Err(Refusal::at(
                    &src.name,
                    b,
                    &src.blocks[0].label,
                    v,
                    Cause::BranchingControlFlow {
                        blocks: src.blocks.len(),
                    },
                ));
            }
        }
    }

    // ---- hand the gradients back ----------------------------------------
    // Each active parameter's gradient is written at its own parameter index,
    // which is what `Job::active_slots` tells the caller to expect.
    let grad = out.push(b, Op::Param(grad_param), Ty::Ptr);
    for i in &active_params {
        let source_param = src.blocks[0]
            .insts
            .iter()
            .copied()
            .find(|&sv| matches!(*src.op(sv), Op::Param(j) if j == *i));
        let g = source_param
            .and_then(|sv| adj.get(&sv).copied())
            .unwrap_or(zero);
        let idx = out.push(b, Op::ConstInt(i64::from(*i)), Ty::Int);
        let slot = out.push(b, Op::Gep(grad, idx), Ty::Ptr);
        out.push(b, Op::Store(slot, g), Ty::Void);
    }
    stats.adjoint_insts = out.insts.len() - before_sweep;

    out.blocks[0].term = Term::Ret(Some(match returned {
        Some(r) => pmap[&r],
        None => zero,
    }));

    let new_id = module.add(out);
    Ok((new_id, stats))
}

/// `adj[target] += contribution`, creating the entry when it is the first one.
///
/// The first contribution is stored rather than added to a zero, so a value
/// reached once -- which is most of them -- costs no instruction at all.
fn accumulate(
    out: &mut Func,
    b: BlockId,
    adj: &mut HashMap<Value, Value>,
    target: Value,
    contribution: Value,
) {
    match adj.get(&target).copied() {
        Some(existing) => {
            let sum = out.push(b, Op::Bin(BinOp::Add, existing, contribution), Ty::Real);
            adj.insert(target, sum);
        }
        None => {
            adj.insert(target, contribution);
        }
    }
}

/// `g * d/da op(a)`, reusing the primal result wherever the derivative is a
/// function of it, for the same reason forward mode does: recomputing `exp`
/// to differentiate `exp` doubles the transcendental count for nothing.
fn unary_adjoint(out: &mut Func, b: BlockId, o: UnOp, pa: Value, primal: Value, g: Value) -> Value {
    let r = |out: &mut Func, op: Op| out.push(b, op, Ty::Real);
    let mul = |out: &mut Func, x: Value, y: Value| out.push(b, Op::Bin(BinOp::Mul, x, y), Ty::Real);
    match o {
        UnOp::Neg => r(out, Op::Un(UnOp::Neg, g)),
        UnOp::Sin => {
            let c = r(out, Op::Un(UnOp::Cos, pa));
            mul(out, g, c)
        }
        UnOp::Cos => {
            let s = r(out, Op::Un(UnOp::Sin, pa));
            let n = r(out, Op::Un(UnOp::Neg, s));
            mul(out, g, n)
        }
        UnOp::Tan => {
            let sq = mul(out, primal, primal);
            let one = r(out, Op::ConstReal(1.0));
            let s = r(out, Op::Bin(BinOp::Add, one, sq));
            mul(out, g, s)
        }
        UnOp::Exp => mul(out, g, primal),
        UnOp::Log => r(out, Op::Bin(BinOp::Div, g, pa)),
        UnOp::Sqrt => {
            let two = r(out, Op::ConstReal(2.0));
            let d = mul(out, two, primal);
            r(out, Op::Bin(BinOp::Div, g, d))
        }
        UnOp::Tanh => {
            let sq = mul(out, primal, primal);
            let one = r(out, Op::ConstReal(1.0));
            let s = r(out, Op::Bin(BinOp::Sub, one, sq));
            mul(out, g, s)
        }
        UnOp::Sinh => {
            let c = r(out, Op::Un(UnOp::Cosh, pa));
            mul(out, g, c)
        }
        UnOp::Cosh => {
            let s = r(out, Op::Un(UnOp::Sinh, pa));
            mul(out, g, s)
        }
        UnOp::Abs => {
            // The right derivative at zero, the same convention forward mode
            // takes. Written down in both places rather than discovered.
            let z = r(out, Op::ConstReal(0.0));
            let ge = out.push(b, Op::FCmp(Pred::Ge, pa, z), Ty::Int);
            let neg = r(out, Op::Un(UnOp::Neg, g));
            r(out, Op::Select(ge, g, neg))
        }
        UnOp::Recip => {
            let sq = mul(out, primal, primal);
            let m = mul(out, g, sq);
            r(out, Op::Un(UnOp::Neg, m))
        }
        UnOp::Erf => {
            let sq = mul(out, pa, pa);
            let n = r(out, Op::Un(UnOp::Neg, sq));
            let e = r(out, Op::Un(UnOp::Exp, n));
            let c = r(out, Op::ConstReal(std::f64::consts::FRAC_2_SQRT_PI));
            let k = mul(out, c, e);
            mul(out, g, k)
        }
    }
}

/// The adjoint contributions to both operands of a binary operation.
fn binary_adjoint(
    out: &mut Func,
    b: BlockId,
    o: BinOp,
    pa: Value,
    pb: Value,
    primal: Value,
    g: Value,
) -> (Value, Value) {
    let r = |out: &mut Func, op: Op| out.push(b, op, Ty::Real);
    let mul = |out: &mut Func, x: Value, y: Value| out.push(b, Op::Bin(BinOp::Mul, x, y), Ty::Real);
    match o {
        BinOp::Add => (g, g),
        BinOp::Sub => {
            let n = r(out, Op::Un(UnOp::Neg, g));
            (g, n)
        }
        BinOp::Mul => {
            let ca = mul(out, g, pb);
            let cb = mul(out, g, pa);
            (ca, cb)
        }
        BinOp::Div => {
            // d/da (a/b) = 1/b, and d/db (a/b) = -(a/b)/b, which reuses the
            // quotient already computed rather than dividing by b squared.
            let ca = r(out, Op::Bin(BinOp::Div, g, pb));
            let q = mul(out, g, primal);
            let d = r(out, Op::Bin(BinOp::Div, q, pb));
            let cb = r(out, Op::Un(UnOp::Neg, d));
            (ca, cb)
        }
        BinOp::Pow => {
            // d/da (a^b) = b * a^(b-1), as its own power: the shorter
            // (a^b) * b / a is 0/0 at a = 0 and made x^2 undifferentiable at
            // the origin. d/db (a^b) = a^b * log a, with log a guarded at
            // a <= 0, where the product has limit 0 for b > 0.
            let one = r(out, Op::ConstReal(1.0));
            let bm1 = r(out, Op::Bin(BinOp::Sub, pb, one));
            let apow = r(out, Op::Bin(BinOp::Pow, pa, bm1));
            let da = mul(out, pb, apow);
            let ca = mul(out, g, da);
            let z = r(out, Op::ConstReal(0.0));
            let positive = out.push(b, Op::FCmp(Pred::Gt, pa, z), Ty::Int);
            let lg = r(out, Op::Un(UnOp::Log, pa));
            let lg = r(out, Op::Select(positive, lg, z));
            let db = mul(out, primal, lg);
            let cb = mul(out, g, db);
            (ca, cb)
        }
        BinOp::Max => {
            let z = r(out, Op::ConstReal(0.0));
            let ge = out.push(b, Op::FCmp(Pred::Ge, pa, pb), Ty::Int);
            let ca = r(out, Op::Select(ge, g, z));
            let cb = r(out, Op::Select(ge, z, g));
            (ca, cb)
        }
        BinOp::Min => {
            let z = r(out, Op::ConstReal(0.0));
            let le = out.push(b, Op::FCmp(Pred::Le, pa, pb), Ty::Int);
            let ca = r(out, Op::Select(le, g, z));
            let cb = r(out, Op::Select(le, z, g));
            (ca, cb)
        }
        BinOp::Atan2 => {
            // d/da atan2(a, b) = b / (a^2 + b^2), d/db = -a / (a^2 + b^2).
            let a2 = mul(out, pa, pa);
            let b2 = mul(out, pb, pb);
            let den = r(out, Op::Bin(BinOp::Add, a2, b2));
            let gb = mul(out, g, pb);
            let ca = r(out, Op::Bin(BinOp::Div, gb, den));
            let ga = mul(out, g, pa);
            let d = r(out, Op::Bin(BinOp::Div, ga, den));
            let cb = r(out, Op::Un(UnOp::Neg, d));
            (ca, cb)
        }
    }
}
