//! Forward mode, checked against the only oracle that does not share its code:
//! central differences on the primal.
//!
//! Central differences are second-order accurate, so with h = 1e-5 the
//! truncation error is around 1e-10 and the cancellation error around 1e-11.
//! A relative tolerance of 1e-6 is therefore loose enough never to fire on
//! correct code and tight enough to catch a wrong chain rule term, which is
//! what a wrong AD rule looks like.

use catalyst::activity::Activity;
use catalyst::forward::{fwddiff, Job};
use catalyst::ir::{self, FuncId, Module, Ty};
use catalyst::opt::{optimise, Level};
use catalyst::programs;
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
        "{what}: forward mode gave {got}, central differences {want}"
    );
}

/// The scalar programs, both optimisation levels, every input direction.
#[test]
fn a_scalar_tangent_matches_central_differences() {
    for level in [Level::None, Level::Full] {
        let mut m = Module::new();
        let poly = programs::poly(&mut m);
        optimise(&mut m, poly, level);
        ir::verify(&m, poly).unwrap();

        let job = Job::new(vec![Activity::Active, Activity::Active]);
        let (d, _) = fwddiff(&mut m, poly, &job);
        ir::verify(&m, d).unwrap();

        for (x, y) in [(0.7, 1.3), (-1.1, 0.4), (2.0, -0.9)] {
            // seed (1,0) then (0,1)
            let mut i1 = Interp::new(&m);
            let dx = i1.call_real(d, &[x, 1.0, y, 0.0]).unwrap();
            let mut i2 = Interp::new(&m);
            let dy = i2.call_real(d, &[x, 0.0, y, 1.0]).unwrap();
            close(
                dx,
                central(&m, poly, &[x, y], 0),
                &format!("poly d/dx at {x},{y} level {level:?}"),
            );
            close(
                dy,
                central(&m, poly, &[x, y], 1),
                &format!("poly d/dy at {x},{y} level {level:?}"),
            );
        }
    }
}

/// A loop whose accumulator starts in memory. At `Level::None` the tangent has
/// to go through a shadow `alloca`; at `Level::Full` mem2reg has removed the
/// alloca and the same derivative comes out of registers. Both must agree.
#[test]
fn a_loop_with_an_accumulator_in_memory_differentiates_either_way() {
    for level in [Level::None, Level::Full] {
        let mut m = Module::new();
        let f = programs::loop_sum(&mut m);
        optimise(&mut m, f, level);
        ir::verify(&m, f).unwrap();

        let job = Job::new(vec![Activity::Active, Activity::Const]);
        let (d, stats) = fwddiff(&mut m, f, &job);
        ir::verify(&m, d).unwrap();
        if level == Level::Full {
            assert_eq!(
                stats.shadow_allocas, 0,
                "mem2reg should have removed the accumulator, leaving no shadow to allocate"
            );
        }

        for x in [0.3, 1.7, -0.8] {
            for n in [0.0, 1.0, 5.0] {
                let mut it = Interp::new(&m);
                let got = it.call_real(d, &[x, 1.0, n]).unwrap();
                let want = central(&m, f, &[x, n], 0);
                close(
                    got,
                    want,
                    &format!("loop_sum d/dx at x={x} n={n} level {level:?}"),
                );
            }
        }
    }
}

/// A reduction over an array. The seed is a shadow buffer, so this also tests
/// that shadow memory is addressed identically to the primal.
#[test]
fn a_reduction_over_an_array_differentiates_through_shadow_memory() {
    for level in [Level::None, Level::Full] {
        let mut m = Module::new();
        let f = programs::logsumexp(&mut m);
        optimise(&mut m, f, level);

        let job = Job::new(vec![Activity::Duplicated, Activity::Const]).hint(0, Prim::Real);
        let (d, _) = fwddiff(&mut m, f, &job);
        ir::verify(&m, d).unwrap();

        let data = [0.5, -1.2, 2.3, 0.1, 1.9];
        for k in 0..data.len() {
            let mut mem = Memory::default();
            let p = mem.place(&data);
            let mut seed = vec![0.0; data.len()];
            seed[k] = 1.0;
            let s = mem.place(&seed);
            let mut it = Interp::with_memory(&m, mem);
            let got = f64::from_bits(it.call(d, &[p, s, data.len() as u64]).unwrap());

            // central difference in the k-th component
            let h = 1e-5;
            let mut hi = data;
            let mut lo = data;
            hi[k] += h;
            lo[k] -= h;
            let mut m1 = Memory::default();
            let ph = m1.place(&hi);
            let mut i1 = Interp::with_memory(&m, m1);
            let a = f64::from_bits(i1.call(f, &[ph, data.len() as u64]).unwrap());
            let mut m2 = Memory::default();
            let pl = m2.place(&lo);
            let mut i2 = Interp::with_memory(&m, m2);
            let b = f64::from_bits(i2.call(f, &[pl, data.len() as u64]).unwrap());
            close(
                got,
                (a - b) / (2.0 * h),
                &format!("logsumexp d/da{k} level {level:?}"),
            );
        }
    }
}

/// The inactive part of the program gets no tangent at all. This is the claim
/// activity analysis exists to make, so it is asserted as a number rather than
/// described in a comment.
#[test]
fn inactive_instructions_get_no_tangent() {
    let mut m = Module::new();
    let f = programs::mlp(&mut m);
    optimise(&mut m, f, Level::Full);
    let job = Job::new(vec![
        Activity::Const,
        Activity::Duplicated,
        Activity::Const,
        Activity::Const,
    ])
    .hint(0, Prim::Real)
    .hint(1, Prim::Real);
    let (d, stats) = fwddiff(&mut m, f, &job);
    ir::verify(&m, d).unwrap();
    assert!(
        stats.active_insts < stats.primal_insts,
        "every instruction was called active, so the analysis decided nothing: \
         {} of {}",
        stats.active_insts,
        stats.primal_insts
    );
    // The index arithmetic -- geps, counters, comparisons -- is the bulk of
    // this function and none of it can carry a derivative.
    assert!(
        stats.active_insts * 2 < stats.primal_insts,
        "expected fewer than half the instructions to be active, got {} of {}",
        stats.active_insts,
        stats.primal_insts
    );
}

/// Type analysis, round-tripped: erase every load type and require the analysis
/// to put back exactly what was there.
#[test]
fn erasing_every_load_type_loses_nothing() {
    let (mut m, names) = programs::suite();
    for (name, id) in names {
        let hints = std::collections::HashMap::from([(0u32, Prim::Real), (1u32, Prim::Real)]);
        let before: Vec<(u32, Ty)> = {
            let f = m.get(id);
            f.live_insts()
                .into_iter()
                .filter(|v| matches!(f.op(*v), catalyst::ir::Op::Load(_)))
                .map(|v| (v, f.ty(v)))
                .collect()
        };
        catalyst::typeanalysis::erase_load_types(m.get_mut(id));
        let info = catalyst::typeanalysis::infer(&m, id, &hints);
        catalyst::typeanalysis::apply(&mut m, id, &info);
        let f = m.get(id);
        for (v, was) in before {
            assert_eq!(
                f.ty(v),
                was,
                "{name}: load %{v} was {was:?} before erasure and {:?} after inference",
                f.ty(v)
            );
        }
        assert!(
            info.conflicts().is_empty(),
            "{name}: type analysis found contradictory evidence for {:?}",
            info.conflicts()
        );
    }
}
