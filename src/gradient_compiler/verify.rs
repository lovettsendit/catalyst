//! Catalyst's verifier: central finite differences over the primal.
//!
//! `docs/interface.md` §12.5. The Catalyst Gradient Compiler proposes a
//! derivative; this file decides whether it is one. It never produces a
//! derivative itself. It evaluates the *primal* -- the lowered, optimised
//! function, with the derivative transform nowhere in the path -- at points
//! either side of each input and forms `(f(x + h) - f(x - h)) / 2h`, which
//! is a fact about the program that owes nothing to the transform.
//!
//! # The points
//!
//! The declared point, and for each requested input that input moved up and
//! down by one per cent (by 0.01 when it is zero), the others held. A
//! derivative that is right at one point and wrong beside it is a derivative
//! with a mistake in it, and one point would not see the mistake.
//!
//! # The step
//!
//! `h = 1e-5 * max(|x|, 1)`. The central difference's truncation error is
//! about `h^2 f'''/6` and its rounding error about `eps |f| / h`; with `h`
//! near 1e-5 both sit around 1e-10 relative for the smooth functions this
//! phase differentiates, well inside the 1e-6 the artifact declares.
//!
//! # What agreement means
//!
//! Within relative `1e-6` or absolute `1e-9`, whichever is looser at the
//! magnitude in question. Two numbers that are both not finite agree with
//! each other, because "no number" matches "no number" and a derivative that
//! is infinite where the estimate is infinite is not wrong; a finite number
//! against a non-finite one never agrees.

use super::engine::Derivative;
use crate::interp::Fault;

pub const RELATIVE: f64 = 1e-6;
pub const ABSOLUTE: f64 = 1e-9;

/// One validated point.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub name: String,
    /// Every parameter, in declaration order.
    pub at: Vec<f64>,
    pub value: f64,
    /// The derivative's partials, in request order.
    pub gradient: Vec<f64>,
    /// The verifier's partials, in the same order.
    pub estimate: Vec<f64>,
    pub agrees: bool,
}

/// Why validation stopped.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// The derivative and the estimate disagree: which input, at which point,
    /// and the two numbers.
    Disagrees {
        input: String,
        point: Vec<f64>,
        derivative: f64,
        estimate: f64,
        case: String,
    },
    /// The program faulted while being run.
    Faulted(Fault),
}

pub fn close(a: f64, b: f64) -> bool {
    if !a.is_finite() || !b.is_finite() {
        return !a.is_finite() && !b.is_finite();
    }
    let gap = (a - b).abs();
    gap <= ABSOLUTE || gap <= RELATIVE * a.abs().max(b.abs())
}

/// The points the verifier visits: the declared one, then each input up and
/// down.
pub fn points(at: &[f64], active: &[usize], inputs: &[String]) -> Vec<(String, Vec<f64>)> {
    let mut out = vec![("declared".to_owned(), at.to_vec())];
    for (&index, name) in active.iter().zip(inputs) {
        let x = at[index];
        let step = if x == 0.0 { 0.01 } else { x.abs() * 0.01 };
        for (suffix, sign) in [("up", 1.0), ("down", -1.0)] {
            let mut point = at.to_vec();
            point[index] = x + sign * step;
            out.push((format!("{name}-{suffix}"), point));
        }
    }
    out
}

/// The central-difference estimate of the partial along parameter `index`.
pub fn estimate(d: &Derivative, at: &[f64], index: usize) -> Result<f64, Fault> {
    let x = at[index];
    let h = 1e-5 * x.abs().max(1.0);
    let mut up = at.to_vec();
    up[index] = x + h;
    let mut down = at.to_vec();
    down[index] = x - h;
    let above = d.primal_value(&up)?;
    let below = d.primal_value(&down)?;
    Ok((above - below) / (2.0 * h))
}

/// Validate the derivative at the declared point and around it.
pub fn validate(d: &Derivative, at: &[f64], inputs: &[String]) -> Result<Vec<Case>, Verdict> {
    let mut cases = Vec::new();
    for (name, point) in points(at, &d.active, inputs) {
        let (value, gradient) = d.evaluate(&point).map_err(Verdict::Faulted)?;
        let mut estimates = Vec::with_capacity(d.active.len());
        for &index in &d.active {
            estimates.push(estimate(d, &point, index).map_err(Verdict::Faulted)?);
        }
        for (k, (&g, &e)) in gradient.iter().zip(&estimates).enumerate() {
            if !close(g, e) {
                return Err(Verdict::Disagrees {
                    input: inputs[k].clone(),
                    point: point.clone(),
                    derivative: g,
                    estimate: e,
                    case: name,
                });
            }
        }
        cases.push(Case {
            name,
            at: point,
            value,
            gradient,
            estimate: estimates,
            agrees: true,
        });
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tolerance_rule_is_the_declared_one() {
        assert!(close(1.0, 1.0 + 5e-7));
        assert!(!close(1.0, 1.0 + 5e-6));
        assert!(close(0.0, 5e-10));
        assert!(!close(0.0, 5e-9));
        assert!(
            close(f64::NAN, f64::INFINITY),
            "no number matches no number"
        );
        assert!(!close(1.0, f64::NAN));
    }

    #[test]
    fn the_points_are_the_declared_one_and_one_per_cent_either_side() {
        let pts = points(&[2.0, 0.0, 7.0], &[0, 1], &["x".to_owned(), "z".to_owned()]);
        assert_eq!(pts.len(), 5);
        assert_eq!(pts[0].0, "declared");
        assert_eq!(pts[1], ("x-up".to_owned(), vec![2.02, 0.0, 7.0]));
        assert_eq!(pts[2], ("x-down".to_owned(), vec![1.98, 0.0, 7.0]));
        assert_eq!(pts[3], ("z-up".to_owned(), vec![2.0, 0.01, 7.0]));
        assert_eq!(pts[4].1, vec![2.0, -0.01, 7.0]);
    }
}
