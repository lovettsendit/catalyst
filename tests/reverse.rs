//! Reverse mode, checked against two oracles that do not share its code:
//! central differences on the primal, and forward mode.
//!
//! Two oracles rather than one, deliberately. Central differences share no code
//! with any transform, so they catch a wrong rule; but they are approximate,
//! and a tolerance loose enough to admit them could in principle admit a small
//! systematic error too. Forward mode is exact and independent -- it was
//! written from the same maths but not the same code, and its chain rule runs
//! in the opposite direction -- so agreement with BOTH is a much stronger
//! statement than agreement with either.
//!
//! The tolerance is the same 1e-6 forward mode uses, for the same reason: with
//! h = 1e-5 central differences carry about 1e-10 of truncation error, so 1e-6
//! never fires on correct code and still catches a dropped chain rule term.

use catalyst::activity::Activity;
use catalyst::build::Builder;
use catalyst::forward::{fwddiff, Job};
use catalyst::ir::{self, BinOp, Func, FuncId, Module, Ty, UnOp};
use catalyst::opt::{optimise, Level};
use catalyst::programs;
use catalyst::refuse::Cause;
use catalyst::reverse::revdiff;
use catalyst::typeanalysis::Prim;
use catalyst::{Interp, Memory};

fn central(module: &Module, id: FuncId, args: &[f64], wrt: usize) -> f64 {
    let h = 1e-5 * args[wrt].abs().max(1.0);
    let mut hi = args.to_vec();
    let mut lo = args.to_vec();
    hi[wrt] += h;
    lo[wrt] -= h;
    let mut a = Interp::new(module);
    let mut b = Interp::new(module);
    (a.call_real(id, &hi).unwrap() - b.call_real(id, &lo).unwrap()) / (2.0 * h)
}

fn close(got: f64, want: f64, what: &str) {
    let scale = got.abs().max(want.abs()).max(1.0);
    assert!(
        (got - want).abs() / scale < 1e-6,
        "{what}: reverse mode gave {got}, the oracle {want}"
    );
}

/// Call a reverse-mode function and read back every gradient it wrote.
///
/// The buffer is sized by the primal's parameter count rather than by the
/// number of active ones, because the gradient of parameter `i` is written at
/// index `i` -- that is the contract `Job::active_slots` describes, and sizing
/// it any other way here would be a test that agreed with itself.
fn gradients(m: &Module, d: FuncId, scalars: &[f64]) -> (f64, Vec<f64>) {
    let mut mem = Memory::default();
    let grad = mem.place(&vec![0.0; scalars.len()]);
    let mut args: Vec<u64> = scalars.iter().map(|x| x.to_bits()).collect();
    args.push(grad);
    args.push(1.0f64.to_bits());
    let mut it = Interp::with_memory(m, mem);
    let primal = f64::from_bits(it.call(d, &args).unwrap());
    (primal, it.mem.read(grad, scalars.len()))
}

/// The headline claim: one reverse pass gives every input's derivative, and
/// each one matches a difference quotient that shares none of its code.
#[test]
fn a_reverse_gradient_matches_central_differences() {
    for level in [Level::None, Level::Full] {
        let mut m = Module::new();
        let poly = programs::poly(&mut m);
        optimise(&mut m, poly, level);
        ir::verify(&m, poly).unwrap();

        let job = Job::new(vec![Activity::Active, Activity::Active]);
        let (d, _) = revdiff(&mut m, poly, &job).expect("poly is straight-line");
        ir::verify(&m, d).unwrap();

        for (x, y) in [(0.7, 1.3), (-1.1, 0.4), (2.0, -0.9)] {
            let (value, g) = gradients(&m, d, &[x, y]);

            let mut check = Interp::new(&m);
            close(
                value,
                check.call_real(poly, &[x, y]).unwrap(),
                &format!("poly value at {x},{y} level {level:?}"),
            );
            close(
                g[0],
                central(&m, poly, &[x, y], 0),
                &format!("poly d/dx at {x},{y} level {level:?}"),
            );
            close(
                g[1],
                central(&m, poly, &[x, y], 1),
                &format!("poly d/dy at {x},{y} level {level:?}"),
            );
        }
    }
}

/// The two transforms are independent implementations of the same mathematics,
/// running the chain rule in opposite directions. Where both apply they must
/// agree exactly enough that no rule can be wrong in only one of them.
#[test]
fn reverse_and_forward_agree_on_the_same_program() {
    let mut m = Module::new();
    let poly = programs::poly(&mut m);
    optimise(&mut m, poly, Level::Full);

    let job = Job::new(vec![Activity::Active, Activity::Active]);
    let (fwd, _) = fwddiff(&mut m, poly, &job);
    let (rev, _) = revdiff(&mut m, poly, &job).expect("poly is straight-line");

    for (x, y) in [(0.7, 1.3), (-1.1, 0.4), (2.0, -0.9), (0.05, 3.0)] {
        let (_, g) = gradients(&m, rev, &[x, y]);
        let mut i1 = Interp::new(&m);
        let dx = i1.call_real(fwd, &[x, 1.0, y, 0.0]).unwrap();
        let mut i2 = Interp::new(&m);
        let dy = i2.call_real(fwd, &[x, 0.0, y, 1.0]).unwrap();
        close(g[0], dx, &format!("d/dx at {x},{y}"));
        close(g[1], dy, &format!("d/dy at {x},{y}"));
    }
}

/// Every unary and binary rule, one at a time, so a wrong rule names itself
/// instead of hiding inside a larger expression that happens to cancel it.
#[test]
fn every_rule_matches_central_differences_on_its_own() {
    let unary = [
        UnOp::Neg,
        UnOp::Sin,
        UnOp::Cos,
        UnOp::Tan,
        UnOp::Exp,
        UnOp::Log,
        UnOp::Sqrt,
        UnOp::Tanh,
        UnOp::Sinh,
        UnOp::Cosh,
        UnOp::Abs,
        UnOp::Recip,
        UnOp::Erf,
    ];
    for o in unary {
        let mut m = Module::new();
        let mut f = Func::new(&format!("u_{}", o.name()), vec![Ty::Real], Ty::Real);
        {
            let mut b = Builder::new(&mut f);
            let x = b.param(0);
            let r = b.un(o, x);
            b.ret(r);
        }
        let id = m.add(f);
        let job = Job::new(vec![Activity::Active]);
        let (d, _) = revdiff(&mut m, id, &job).expect("straight-line");
        ir::verify(&m, d).unwrap();
        // Away from 0 and 1, where log, sqrt and abs have their own trouble.
        for x in [0.37, 1.42, 2.6] {
            let (_, g) = gradients(&m, d, &[x]);
            close(
                g[0],
                central(&m, id, &[x], 0),
                &format!("{} at {x}", o.name()),
            );
        }
    }

    let binary = [
        BinOp::Add,
        BinOp::Sub,
        BinOp::Mul,
        BinOp::Div,
        BinOp::Pow,
        BinOp::Max,
        BinOp::Min,
        BinOp::Atan2,
    ];
    for o in binary {
        let mut m = Module::new();
        let mut f = Func::new(
            &format!("b_{}", o.name()),
            vec![Ty::Real, Ty::Real],
            Ty::Real,
        );
        {
            let mut b = Builder::new(&mut f);
            let x = b.param(0);
            let y = b.param(1);
            let r = b.bin(o, x, y);
            b.ret(r);
        }
        let id = m.add(f);
        let job = Job::new(vec![Activity::Active, Activity::Active]);
        let (d, _) = revdiff(&mut m, id, &job).expect("straight-line");
        ir::verify(&m, d).unwrap();
        // Distinct magnitudes, so Max and Min are not evaluated at a tie where
        // neither one-sided derivative is the answer.
        for (x, y) in [(1.3, 0.7), (2.1, 0.4)] {
            let (_, g) = gradients(&m, d, &[x, y]);
            close(
                g[0],
                central(&m, id, &[x, y], 0),
                &format!("{} d/dx at {x},{y}", o.name()),
            );
            close(
                g[1],
                central(&m, id, &[x, y], 1),
                &format!("{} d/dy at {x},{y}", o.name()),
            );
        }
    }
}

/// Gradients reach a caller-supplied shadow buffer, accumulating into it. This
/// is the `Duplicated` half of the boundary, and it is what a framework calling
/// into Catalyst actually uses.
#[test]
fn gradients_accumulate_into_a_duplicated_shadow() {
    // w(a) = a0*a0 + 3*a1 + sin(a2), straight-line, reading through a pointer.
    let mut m = Module::new();
    let mut f = Func::new("weighted", vec![Ty::Ptr], Ty::Real);
    {
        let mut b = Builder::new(&mut f);
        let p = b.param(0);
        let i0 = b.int(0);
        let i1 = b.int(1);
        let i2 = b.int(2);
        let g0 = b.gep(p, i0);
        let g1 = b.gep(p, i1);
        let g2 = b.gep(p, i2);
        let a0 = b.load(g0, Ty::Real);
        let a1 = b.load(g1, Ty::Real);
        let a2 = b.load(g2, Ty::Real);
        let sq = b.mul(a0, a0);
        let three = b.real(3.0);
        let lin = b.mul(three, a1);
        let s = b.un(UnOp::Sin, a2);
        let t = b.add(sq, lin);
        let out = b.add(t, s);
        b.ret(out);
    }
    let id = m.add(f);

    let job = Job::new(vec![Activity::Duplicated]).hint(0, Prim::Real);
    let (d, stats) = revdiff(&mut m, id, &job).expect("straight-line");
    ir::verify(&m, d).unwrap();
    assert!(
        stats.adjoint_insts > 0,
        "a gradient that cost no instructions was not computed"
    );

    let data = [1.7, -0.4, 0.9];
    let mut mem = Memory::default();
    let p = mem.place(&data);
    let shadow = mem.place(&[0.0; 3]);
    let grad = mem.place(&[0.0]);
    let mut it = Interp::with_memory(&m, mem);
    it.call(d, &[p, shadow, grad, 1.0f64.to_bits()]).unwrap();
    let got = it.mem.read(shadow, 3);

    // d/da0 = 2*a0, d/da1 = 3, d/da2 = cos(a2)
    close(got[0], 2.0 * data[0], "d/da0");
    close(got[1], 3.0, "d/da1");
    close(got[2], data[2].cos(), "d/da2");
}

/// The reliability claim, as a test rather than a sentence: a construct this
/// build cannot differentiate correctly is REFUSED, and the refusal names it.
///
/// If this test ever fails by returning `Ok`, the failure is not that a feature
/// is missing. It is that a wrong gradient is being handed to a caller who
/// asked for a right one.
#[test]
fn a_loop_is_refused_by_name_rather_than_differentiated_wrong() {
    let mut m = Module::new();
    let f = programs::loop_sum(&mut m);
    optimise(&mut m, f, Level::Full);

    let job = Job::new(vec![Activity::Active, Activity::Const]);
    let refusal = revdiff(&mut m, f, &job).expect_err("a loop must be refused, never approximated");

    assert_eq!(refusal.code(), "catalyst.branching_control_flow");
    assert!(matches!(*refusal.cause, Cause::BranchingControlFlow { .. }));
    assert_eq!(refusal.function, "loop_sum");
    assert!(
        refusal.cause.remedy().contains("forward mode"),
        "the remedy must point at the mode that CAN do this, got: {}",
        refusal.cause.remedy()
    );

    // And the remedy is real, not just wording: forward mode does handle it.
    let (d, _) = fwddiff(&mut m, f, &job);
    ir::verify(&m, d).unwrap();
    let mut it = Interp::new(&m);
    let got = it.call_real(d, &[0.3, 1.0, 5.0]).unwrap();
    close(
        got,
        central(&m, f, &[0.3, 5.0], 0),
        "forward mode over the loop",
    );
}

/// A job that disagrees with itself is refused before any code is generated,
/// because the code would be wrong in a way that reads as a modelling mistake
/// rather than a declaration one.
#[test]
fn a_job_that_contradicts_the_signature_is_refused_before_any_code_exists() {
    let mut m = Module::new();
    let poly = programs::poly(&mut m);
    let functions_before = m.funcs.len();

    // poly takes two reals; Duplicated is a pointer annotation.
    let job = Job::new(vec![Activity::Duplicated, Activity::Active]);
    let refusal = revdiff(&mut m, poly, &job).expect_err("the annotation is wrong");
    assert_eq!(refusal.code(), "catalyst.annotation_mismatch");
    assert_eq!(
        m.funcs.len(),
        functions_before,
        "a refused job must leave no half-built function behind"
    );

    // Asking for nothing is refused too, rather than answered with zeros.
    let nothing = Job::new(vec![Activity::Const, Activity::Const]);
    let refusal = revdiff(&mut m, poly, &nothing).expect_err("nothing was active");
    assert_eq!(refusal.code(), "catalyst.nothing_active");
}

/// The refusal is meant to be read by a program as often as by a person, so
/// what it renders is checked as data rather than eyeballed as prose.
#[test]
fn a_refusal_is_machine_readable_and_says_what_to_do() {
    let mut m = Module::new();
    let f = programs::loop_sum(&mut m);
    let job = Job::new(vec![Activity::Active, Activity::Const]);
    let refusal = revdiff(&mut m, f, &job).expect_err("a loop is refused");

    let json = refusal.to_json();
    for required in [
        "\"code\":\"catalyst.branching_control_flow\"",
        "\"function\":\"loop_sum\"",
        "\"detail\":\"",
        "\"remedy\":\"",
    ] {
        assert!(json.contains(required), "{required} missing from {json}");
    }
    assert!(
        json.starts_with('{') && json.ends_with('}'),
        "not an object: {json}"
    );
    // Balanced braces and quotes are the cheapest structural check that does
    // not require writing a JSON parser in a test.
    assert_eq!(
        json.chars().filter(|c| *c == '{').count(),
        json.chars().filter(|c| *c == '}').count(),
        "unbalanced braces in {json}"
    );

    // The human rendering names the same code, so a log line and a matched
    // branch can never disagree about what happened.
    let text = refusal.to_string();
    assert!(text.contains("catalyst.branching_control_flow"), "{text}");
    assert!(
        text.contains("Instead:"),
        "a refusal must say what to do: {text}"
    );
}

/// Control characters in a name cannot break the JSON a caller parses. The
/// names in this IR come from whoever built it, so this is an input, not a
/// constant.
#[test]
fn a_hostile_function_name_cannot_break_the_json() {
    let mut m = Module::new();
    let mut f = Func::new(
        "bad\"name\\with\nnewline\tand\u{1}control",
        vec![Ty::Real],
        Ty::Real,
    );
    {
        let mut b = Builder::new(&mut f);
        let x = b.param(0);
        b.ret(x);
    }
    let id = m.add(f);
    let refusal =
        revdiff(&mut m, id, &Job::new(vec![Activity::Const])).expect_err("nothing is active");
    let json = refusal.to_json();

    assert!(json.contains("\\\""), "the quote was not escaped: {json}");
    assert!(
        json.contains("\\\\"),
        "the backslash was not escaped: {json}"
    );
    assert!(json.contains("\\n"), "the newline was not escaped: {json}");
    assert!(json.contains("\\t"), "the tab was not escaped: {json}");
    assert!(
        json.contains("\\u0001"),
        "the control character was not escaped: {json}"
    );
    assert!(
        !json.contains('\n') && !json.contains('\u{1}'),
        "a raw control character survived into the output: {json:?}"
    );
}
