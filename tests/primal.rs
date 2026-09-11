//! The primal operations compute what they claim to compute.
//!
//! Differentiation is only ever as good as the function being differentiated.
//! An AD rule can be exactly right and still produce a number nobody wants, if
//! the primal it is paired with is an approximation of something else -- and
//! that failure is invisible to every test that checks the derivative against
//! the same approximate primal. It shows up only against an outside oracle.
//!
//! This file is that oracle for the operations whose implementation is ours
//! rather than the standard library's.

use catalyst::ir::UnOp;

/// `erf`, against values computed to full double precision elsewhere.
///
/// # Why this test exists
///
/// `erf` used to be Abramowitz & Stegun 7.1.26, a four-line approximation with
/// a stated maximum error of **1.5e-7**. Every other real operation in the IR
/// is the standard library's and correct to the last bit or two, so this one
/// was the only place an `f64` pipeline quietly became a single-precision one.
///
/// It was found through differentiation rather than by reading: both AD
/// transforms use the exact analytic rule `2/sqrt(pi) * exp(-x^2)`, so with an
/// approximate primal the transform returned the derivative of a function the
/// interpreter did not compute, and the disagreement with central differences
/// measured 1.5e-6 relative. The AD rule was right; the primal was not.
///
/// The tolerance here is 1e-14 rather than 1e-16 because the reference values
/// are decimal literals and the argument reduction costs a bit or two. It is
/// still eight orders of magnitude tighter than what it replaced, which is the
/// point.
#[test]
fn erf_is_accurate_to_double_precision() {
    // Reference values, correct to the digits shown.
    let cases = [
        (0.0, 0.0),
        (0.1, 0.112_462_916_018_284_89),
        (0.37, 0.399_205_984_042_999_2),
        (0.5, 0.520_499_877_813_046_5),
        (1.0, 0.842_700_792_949_714_9),
        (1.5, 0.966_105_146_475_310_7),
        (2.0, 0.995_322_265_018_952_7),
        (3.0, 0.999_977_909_503_001_4),
        (4.0, 0.999_999_984_582_742_1),
    ];
    for (x, want) in cases {
        for signed in [1.0, -1.0] {
            let got = UnOp::Erf.eval(signed * x);
            let expect = signed * want;
            let scale = expect.abs().max(1.0);
            assert!(
                (got - expect).abs() / scale < 1e-14,
                "erf({}) gave {got}, want {expect} (off by {:e})",
                signed * x,
                (got - expect).abs()
            );
        }
    }
}

/// The two branches of `erf` must agree where they meet, or the crossover is a
/// visible step in a function that has none. The crossover is at `x^2 = 1.5`,
/// so this walks across it in small steps and requires the result to stay
/// monotone and smooth.
#[test]
fn erf_has_no_step_at_the_crossover_between_its_two_branches() {
    let crossover = 1.5_f64.sqrt();
    let mut previous = UnOp::Erf.eval(crossover - 0.01);
    let mut x = crossover - 0.01;
    while x < crossover + 0.01 {
        x += 0.000_5;
        let got = UnOp::Erf.eval(x);
        assert!(
            got > previous,
            "erf is strictly increasing, but erf({x}) = {got} did not exceed {previous}"
        );
        // Over a step of 5e-4 the true change is at most 5e-4 * 2/sqrt(pi),
        // about 5.7e-4. Ten times that would be a discontinuity.
        assert!(
            got - previous < 5.7e-3,
            "erf jumped by {} across {x}, which is a step rather than a slope",
            got - previous
        );
        previous = got;
    }
}

/// The saturating tails are exactly 1 and -1, not 0.9999999 or 1.0000001.
/// A value above 1 would make `1 - erf(x)` negative, which is the kind of
/// thing that turns into a NaN three functions later.
#[test]
fn erf_saturates_without_overshooting() {
    for x in [6.0, 10.0, 30.0, 1e3] {
        assert_eq!(UnOp::Erf.eval(x), 1.0, "erf({x}) must be exactly 1");
        assert_eq!(UnOp::Erf.eval(-x), -1.0, "erf(-{x}) must be exactly -1");
    }
}
