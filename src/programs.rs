//! The programs everything is measured on.
//!
//! One definition each, used by the tests, the benchmarks, the flame graph and
//! the documentation, so that a number quoted anywhere refers to the same code
//! as a number quoted anywhere else. They are chosen to exercise the parts of
//! the pipeline that are hard, not the parts that are easy:
//!
//! | program      | what it is there to break |
//! | ------------ | ------------------------- |
//! | `poly`       | straight-line arithmetic with repeated subexpressions -- GVN, and adjoint accumulation across multiple uses |
//! | `loop_sum`   | a counted loop with an accumulator in an `alloca` -- mem2reg, LICM, and reverse mode over a back edge |
//! | `logsumexp`  | a reduction over an array in memory -- shadow memory, `gep`, and a two-pass numerically stable formulation |
//! | `mlp`        | two dense layers with `tanh` -- what a machine learning caller actually has |
//! | `ode`        | fixed-step RK4 on a stiff-ish system -- long dependency chains, where caching every intermediate is what kills naive reverse mode |

use crate::build::Builder;
use crate::ir::*;

/// A counted loop `for i in 0..n`, built as a proper CFG.
///
/// Returns the header, body and exit blocks together with the induction
/// variable, so the caller fills in the body and this keeps the phi bookkeeping
/// honest.
pub struct CountedLoop {
    pub header: BlockId,
    pub body: BlockId,
    pub exit: BlockId,
    pub index: Value,
}

/// Build `for i in 0..n`, with `carried` values threaded through as phis.
///
/// `carried` is the list of loop-carried values at entry; the returned phis are
/// their values inside the body, and `close` supplies their values at the latch.
pub fn counted_loop(
    b: &mut Builder,
    n: Value,
    carried: &[Value],
) -> (CountedLoop, Vec<Value>, BlockId) {
    let pre = b.at;
    let header = b.block("head");
    let body = b.block("body");
    let exit = b.block("exit");
    b.at(pre);
    b.br(header);

    b.at(header);
    let zero = b.int(0);
    // Placeholder incoming; the latch entry is added by `close_loop`.
    let index = b.phi(vec![(pre, zero)], Ty::Int);
    let phis: Vec<Value> = carried
        .iter()
        .map(|&c| b.phi(vec![(pre, c)], b.f.ty(c)))
        .collect();
    let more = b.icmp(Pred::Lt, index, n);
    b.condbr(more, body, exit);

    b.at(body);
    (
        CountedLoop {
            header,
            body,
            exit,
            index,
        },
        phis,
        header,
    )
}

/// Close a counted loop: bump the index, feed the carried values back, branch.
pub fn close_loop(b: &mut Builder, l: &CountedLoop, phis: &[Value], next: &[Value]) {
    let latch = b.at;
    let one = b.int(1);
    let bumped = b.iadd(l.index, one);
    b.br(l.header);
    if let Op::Phi(inc) = &mut b.f.insts[l.index as usize].op {
        inc.push((latch, bumped));
    }
    for (&phi, &nv) in phis.iter().zip(next.iter()) {
        if let Op::Phi(inc) = &mut b.f.insts[phi as usize].op {
            inc.push((latch, nv));
        }
    }
    b.at(l.exit);
}

/// `p(x, y) = (x*y + sin(x))^2 + exp(x*y) * (x*y + sin(x))`
///
/// `x*y` appears three times and `x*y + sin(x)` twice. Written that way on
/// purpose: without GVN the reverse pass accumulates into three separate
/// adjoints and recomputes the product; with it, once.
pub fn poly(m: &mut Module) -> FuncId {
    let mut f = Func::new("poly", vec![Ty::Real, Ty::Real], Ty::Real);
    {
        let mut b = Builder::new(&mut f);
        let x = b.param(0);
        let y = b.param(1);
        let xy1 = b.mul(x, y);
        let s = b.un(UnOp::Sin, x);
        let t1 = b.add(xy1, s);
        let sq = b.mul(t1, t1);
        let xy2 = b.mul(x, y);
        let e = b.un(UnOp::Exp, xy2);
        let xy3 = b.mul(x, y);
        let s2 = b.un(UnOp::Sin, x);
        let t2 = b.add(xy3, s2);
        let prod = b.mul(e, t2);
        let out = b.add(sq, prod);
        b.ret(out);
    }
    m.add(f)
}

/// `s(x, n) = sum_{i<n} (x^2 * i + tanh(x))`, with the accumulator in memory.
///
/// The accumulator is an `alloca` written every iteration, which is what an
/// unoptimised frontend emits and what mem2reg is for. `x*x` and `tanh(x)` are
/// loop-invariant, which is what LICM is for. Reverse mode over this needs a
/// trip count, which is what the control-flow tape is for.
pub fn loop_sum(m: &mut Module) -> FuncId {
    let mut f = Func::new("loop_sum", vec![Ty::Real, Ty::Int], Ty::Real);
    {
        let mut b = Builder::new(&mut f);
        let x = b.param(0);
        let n = b.param(1);
        let one = b.int(1);
        let acc = b.alloca(one);
        let zero = b.real(0.0);
        b.store(acc, zero);

        let (l, phis, _) = counted_loop(&mut b, n, &[]);
        let _ = phis;
        b.at(l.body);
        let cur = b.load(acc, Ty::Real);
        let x2 = b.mul(x, x);
        let fi = b.sitofp(l.index);
        let term = b.mul(x2, fi);
        let th = b.un(UnOp::Tanh, x);
        let term2 = b.add(term, th);
        let next = b.add(cur, term2);
        b.store(acc, next);
        close_loop(&mut b, &l, &[], &[]);

        b.at(l.exit);
        let out = b.load(acc, Ty::Real);
        b.ret(out);
    }
    m.add(f)
}

/// `logsumexp(p, n) = log(sum_i exp(a_i - m)) + m`, `m = max_i a_i`.
///
/// Two passes over an array in linear memory. The maximum makes it correct for
/// large inputs and gives reverse mode a `select` chain to differentiate; the
/// array makes it need shadow memory rather than shadow registers.
pub fn logsumexp(m: &mut Module) -> FuncId {
    let mut f = Func::new("logsumexp", vec![Ty::Ptr, Ty::Int], Ty::Real);
    {
        let mut b = Builder::new(&mut f);
        let p = b.param(0);
        let n = b.param(1);

        // pass one: the maximum
        let neg_inf = b.real(f64::NEG_INFINITY);
        let (l1, ph1, _) = counted_loop(&mut b, n, &[neg_inf]);
        b.at(l1.body);
        let addr = b.gep(p, l1.index);
        let a = b.load(addr, Ty::Real);
        let best = b.bin(BinOp::Max, ph1[0], a);
        close_loop(&mut b, &l1, &ph1, &[best]);

        b.at(l1.exit);
        let mx = ph1[0];
        // pass two: the sum of shifted exponentials
        let zero = b.real(0.0);
        let (l2, ph2, _) = counted_loop(&mut b, n, &[zero]);
        b.at(l2.body);
        let addr2 = b.gep(p, l2.index);
        let a2 = b.load(addr2, Ty::Real);
        let shifted = b.sub(a2, mx);
        let e = b.un(UnOp::Exp, shifted);
        let acc = b.add(ph2[0], e);
        close_loop(&mut b, &l2, &ph2, &[acc]);

        b.at(l2.exit);
        let total = ph2[0];
        let lg = b.un(UnOp::Log, total);
        let out = b.add(lg, mx);
        b.ret(out);
    }
    m.add(f)
}

/// A two-layer perceptron: `y = w2 . tanh(W1 x + b1) + b2`, summed to a scalar.
///
/// Signature `(x, w, n_in, n_hidden)`: `x` is the input vector, `w` is one flat
/// parameter buffer laid out as `W1` then `b1` then `w2` then `b2`. Flat because
/// that is what a PyTorch or JAX caller hands across a C boundary, and the
/// gradient it wants back has the same shape.
pub fn mlp(m: &mut Module) -> FuncId {
    let mut f = Func::new("mlp", vec![Ty::Ptr, Ty::Ptr, Ty::Int, Ty::Int], Ty::Real);
    {
        let mut b = Builder::new(&mut f);
        let x = b.param(0);
        let w = b.param(1);
        let n_in = b.param(2);
        let n_h = b.param(3);

        let zero = b.real(0.0);
        // for h in 0..n_h
        let (lh, phh, _) = counted_loop(&mut b, n_h, &[zero]);
        b.at(lh.body);
        // row start = h * n_in
        let row = b.iop(IntOp::Mul, lh.index, n_in);
        let z0 = b.real(0.0);
        let (li, phi_i, _) = counted_loop(&mut b, n_in, &[z0]);
        b.at(li.body);
        let off = b.iadd(row, li.index);
        let wa = b.gep(w, off);
        let wv = b.load(wa, Ty::Real);
        let xa = b.gep(x, li.index);
        let xv = b.load(xa, Ty::Real);
        let prod = b.mul(wv, xv);
        let acc_i = b.add(phi_i[0], prod);
        close_loop(&mut b, &li, &phi_i, &[acc_i]);

        b.at(li.exit);
        // bias b1[h] at n_h*n_in + h
        let wsize = b.iop(IntOp::Mul, n_h, n_in);
        let b1off = b.iadd(wsize, lh.index);
        let b1a = b.gep(w, b1off);
        let b1v = b.load(b1a, Ty::Real);
        let pre = b.add(phi_i[0], b1v);
        let act = b.un(UnOp::Tanh, pre);
        // w2[h] at n_h*n_in + n_h + h
        let base2 = b.iadd(wsize, n_h);
        let w2off = b.iadd(base2, lh.index);
        let w2a = b.gep(w, w2off);
        let w2v = b.load(w2a, Ty::Real);
        let contrib = b.mul(act, w2v);
        let acc_h = b.add(phh[0], contrib);
        close_loop(&mut b, &lh, &phh, &[acc_h]);

        b.at(lh.exit);
        // b2 at n_h*n_in + 2*n_h
        let wsize2 = b.iop(IntOp::Mul, n_h, n_in);
        let two = b.int(2);
        let twoh = b.iop(IntOp::Mul, n_h, two);
        let b2off = b.iadd(wsize2, twoh);
        let b2a = b.gep(w, b2off);
        let b2v = b.load(b2a, Ty::Real);
        let out = b.add(phh[0], b2v);
        b.ret(out);
    }
    m.add(f)
}

/// Fixed-step RK4 on `y' = -k y + sin(t)`, returning `y(T)`.
///
/// Signature `(y0, k, steps, dt)`. A long chain where every stage depends on
/// the last: the case where a reverse pass that caches every intermediate uses
/// memory proportional to the number of steps, and the min-cut is worth having.
pub fn ode(m: &mut Module) -> FuncId {
    let mut f = Func::new("ode", vec![Ty::Real, Ty::Real, Ty::Int, Ty::Real], Ty::Real);
    {
        let mut b = Builder::new(&mut f);
        let y0 = b.param(0);
        let k = b.param(1);
        let steps = b.param(2);
        let dt = b.param(3);

        let t0 = b.real(0.0);
        let (l, ph, _) = counted_loop(&mut b, steps, &[y0, t0]);
        b.at(l.body);
        let y = ph[0];
        let t = ph[1];
        let half = b.real(0.5);
        let hdt = b.mul(dt, half);

        // k1 = -k*y + sin(t)
        let ky = b.mul(k, y);
        let nky = b.un(UnOp::Neg, ky);
        let st = b.un(UnOp::Sin, t);
        let k1 = b.add(nky, st);
        // k2 at t + dt/2, y + dt/2*k1
        let y2 = b.mul(hdt, k1);
        let y2 = b.add(y, y2);
        let t2 = b.add(t, hdt);
        let ky2 = b.mul(k, y2);
        let nky2 = b.un(UnOp::Neg, ky2);
        let st2 = b.un(UnOp::Sin, t2);
        let k2 = b.add(nky2, st2);
        // k3
        let y3 = b.mul(hdt, k2);
        let y3 = b.add(y, y3);
        let ky3 = b.mul(k, y3);
        let nky3 = b.un(UnOp::Neg, ky3);
        let k3 = b.add(nky3, st2);
        // k4 at t + dt
        let y4 = b.mul(dt, k3);
        let y4 = b.add(y, y4);
        let t4 = b.add(t, dt);
        let ky4 = b.mul(k, y4);
        let nky4 = b.un(UnOp::Neg, ky4);
        let st4 = b.un(UnOp::Sin, t4);
        let k4 = b.add(nky4, st4);

        let two = b.real(2.0);
        let k2t = b.mul(k2, two);
        let k3t = b.mul(k3, two);
        let s1 = b.add(k1, k2t);
        let s2 = b.add(s1, k3t);
        let s3 = b.add(s2, k4);
        let sixth = b.real(1.0 / 6.0);
        let scaled = b.mul(s3, sixth);
        let step = b.mul(dt, scaled);
        let ynext = b.add(y, step);
        close_loop(&mut b, &l, &ph, &[ynext, t4]);

        b.at(l.exit);
        b.ret(ph[0]);
    }
    m.add(f)
}

/// Everything, in one module, in a fixed order.
pub fn suite() -> (Module, Vec<(&'static str, FuncId)>) {
    let mut m = Module::new();
    let names = vec![
        ("poly", poly(&mut m)),
        ("loop_sum", loop_sum(&mut m)),
        ("logsumexp", logsumexp(&mut m)),
        ("mlp", mlp(&mut m)),
        ("ode", ode(&mut m)),
    ];
    (m, names)
}
