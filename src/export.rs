//! `catalyst export go`: six files that leave the building without Catalyst.
//!
//! `docs/interface.md` §3 lists them. The point of the set is that somebody
//! who has never run this crate can check the claim: `go build`, run the
//! binary against `fixtures.json`, and compare. The fixtures carry the Rust
//! engine's own numbers, so the comparison is against a measurement rather
//! than against the exporter's opinion of itself.
//!
//! # What the fixtures cover, and why those cases
//!
//! * the problem's own inputs, because that is the point the user asked about;
//! * two interior points, at a quarter and three quarters of every range,
//!   because a generator that got one point right by accident rarely gets
//!   three;
//! * both bounds of **every** parameter, because the edges are where a domain
//!   check and a square root disagree;
//! * a point below a minimum, whose documented behaviour is a refusal --
//!   an export that quietly returns a number outside its declared domain is
//!   the failure this case exists to catch.
//!
//! # Byte-for-byte determinism
//!
//! Nothing here reads the clock, the environment or a path. Objects keep the
//! order they were built in, numbers are written in the shortest form that
//! reads back as the same double, and two exports of one problem are the same
//! bytes -- which is what makes an export reviewable in a diff.

use crate::cli::Refused;
use crate::json::{self, obj, s, Json};
use crate::paths::{self, Resolved};
use crate::problem::Problem;
use std::fmt::Write as _;

/// The two sentences every export states, verbatim, in `parameters.json` and
/// in the validation report.
pub const LIMITATION_GO: &str = "Arbitrary Go is not differentiable: only the exported function and its gradient are generated from the engine's IR.";
pub const LIMITATION_FLOAT: &str = "Floating-point results are not identical across languages or platforms: the fixtures declare a tolerance instead of exact equality.";
/// The R export's version of the first sentence. Same claim, other language.
pub const LIMITATION_R: &str = "Arbitrary R is not differentiable: only the exported function and its gradient are generated from the engine's IR.";

/// The comparison the fixtures declare: within relative `RELATIVE` or
/// absolute `ABSOLUTE`, whichever is larger.
pub const RELATIVE: f64 = 1e-9;
pub const ABSOLUTE: f64 = 1e-12;

/// Significant digits an expected value is written to.
///
/// Not seventeen. Seventeen is what a double needs to survive a decimal round
/// trip, and writing it would state a precision this export explicitly does
/// not claim: `LIMITATION_FLOAT` says in the same file that results are not
/// identical across platforms and that the fixtures declare a tolerance
/// instead of exact equality. Thirteen digits keep the written expectation
/// within about 5e-14 of the engine -- two orders inside the 1e-12 a fixture
/// is checked against, four orders inside the 1e-9 it declares -- while
/// saying only as much as the tolerance can support.
///
/// The *inputs* of a case are not rounded. A boundary case has to name its
/// bound exactly, or it is no longer a boundary case.
const EXPECTED_DIGITS: usize = 13;

/// One number as a fixture states it.
fn expected_number(x: f64) -> f64 {
    json::to_significant(x, EXPECTED_DIGITS)
}

/// The files an export writes, in the order they are listed back.
pub const FILES: &[&str] = &[
    "go.mod",
    "function.go",
    "main.go",
    "parameters.json",
    "fixtures.json",
    "validation-report.md",
];

/// The files an R export writes, in the order they are listed back.
pub const R_FILES: &[&str] = &[
    "function.R",
    "parameters.R",
    "fixtures.R",
    "test_function.R",
    "validation-report.md",
];

/// One fixture: a point, and what the engine does there.
struct Case {
    name: String,
    kind: &'static str,
    inputs: Vec<f64>,
    outcome: Outcome,
    /// The engine's own numbers at this point, when it has finite ones.
    ///
    /// This is *not* the same claim as [`Case::outcome`], and the difference
    /// matters at an out-of-domain point: the engine still computes a number
    /// half a range below a declared minimum, and a conforming export must
    /// refuse anyway. Recording the number alongside the refusal says that
    /// out loud, and gives the R export -- which carries no `main` to run --
    /// something to check its arithmetic against on every case it has one for.
    measured: Option<(f64, Vec<f64>)>,
}

enum Outcome {
    /// The engine's value and partials, both finite throughout.
    Measured { value: f64, gradient: Vec<f64> },
    /// The engine's value or one of its partials is not a number here.
    NonFinite,
    /// Outside the declared domain: the documented behaviour is a refusal.
    Refused,
}

/// Write the whole export into an already validated, empty directory.
pub fn write_go_export(problem: &Problem, out: &Resolved) -> Result<usize, Refused> {
    let cases = cases(problem);
    let function = crate::gocode::function_go(problem)?;

    paths::write_file(&out.full, "go.mod", &go_mod(problem))?;
    paths::write_file(&out.full, "function.go", &function)?;
    paths::write_file(&out.full, "main.go", MAIN_GO)?;
    paths::write_file(
        &out.full,
        "parameters.json",
        &parameters(problem).render_pretty(),
    )?;
    paths::write_file(
        &out.full,
        "fixtures.json",
        &fixtures(problem, &cases).render_pretty(),
    )?;
    paths::write_file(&out.full, "validation-report.md", &report(problem, &cases))?;
    Ok(cases.len())
}

/// Write the whole R export into an already validated, empty directory.
///
/// The cases are [`cases`] -- the same function the Go export calls -- and the
/// arithmetic is [`crate::rcode`], which renders the same instruction list
/// [`crate::gocode`] renders. Neither the case list nor the arithmetic is
/// written twice, which is the only way two exports of one problem can be
/// promised to agree rather than observed to agree today.
pub fn write_r_export(problem: &Problem, out: &Resolved) -> Result<usize, Refused> {
    let cases = cases(problem);
    let function = crate::rcode::function_r(problem)?;

    paths::write_file(&out.full, "function.R", &function)?;
    paths::write_file(&out.full, "parameters.R", &parameters_r(problem))?;
    paths::write_file(&out.full, "fixtures.R", &fixtures_r(problem, &cases))?;
    paths::write_file(&out.full, "test_function.R", TEST_FUNCTION_R)?;
    paths::write_file(
        &out.full,
        "validation-report.md",
        &report_r(problem, &cases),
    )?;
    Ok(cases.len())
}

fn parameters_r(problem: &Problem) -> String {
    let mut out = String::new();
    out.push_str(
        "# Generated by `catalyst export r`. The declared parameters, in declaration\n\
         # order, with the point the problem asked about and the range each was\n\
         # declared inside.\n\
         #\n\
         # Base R only: no package is loaded. Do not edit; regenerate the export.\n\n\
         catalyst_parameters <- list(\n",
    );
    let entries: Vec<String> = problem
        .params
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let domain = &problem.domains[i];
            format!(
                "  list(name = {}, value = {}, min = {}, max = {}, unit = {})",
                crate::rcode::r_string(name),
                crate::rcode::literal(problem.inputs[i]),
                crate::rcode::literal(domain.min),
                crate::rcode::literal(domain.max),
                crate::rcode::r_string(&domain.unit)
            )
        })
        .collect();
    out.push_str(&entries.join(",\n"));
    out.push_str("\n)\n\nnames(catalyst_parameters) <- vapply(catalyst_parameters, function(p) p$name, character(1))\n");
    out
}

/// A point as an R named list.
fn point_r(problem: &Problem, at: &[f64]) -> String {
    let fields: Vec<String> = problem
        .params
        .iter()
        .zip(at)
        .map(|(name, value)| {
            format!(
                "{} = {}",
                crate::rcode::r_string(name),
                crate::rcode::literal(*value)
            )
        })
        .collect();
    format!("list({})", fields.join(", "))
}

fn fixtures_r(problem: &Problem, cases: &[Case]) -> String {
    let mut out = String::new();
    // No `value = ` may appear in this preamble: the first one in the file is
    // the first case's, and that is the one a check of the checker corrupts.
    out.push_str(
        "# Generated by `catalyst export r`. The same cases as the Go export's\n\
         # fixtures.json, carrying the same numbers: they are the Rust engine's own\n\
         # measurements, not this file's opinion of them.\n\
         #\n\
         # Each entry carries the engine's numbers when it has finite ones -- including\n\
         # at the out-of-domain point, where they are exactly what a conforming export\n\
         # must refuse to return rather than report.\n\
         #\n\
         # Base R only: no package is loaded. Do not edit; regenerate the export.\n\n",
    );
    let _ = writeln!(
        out,
        "catalyst_tolerance <- list(relative = {}, absolute = {})\n",
        crate::rcode::literal(RELATIVE),
        crate::rcode::literal(ABSOLUTE)
    );
    out.push_str("catalyst_fixtures <- list(\n");
    let entries: Vec<String> = cases
        .iter()
        .map(|c| {
            let mut fields = vec![
                format!("name = {}", crate::rcode::r_string(&c.name)),
                format!("kind = {}", crate::rcode::r_string(c.kind)),
                format!("inputs = {}", point_r(problem, &c.inputs)),
                format!(
                    "refused = {}",
                    if matches!(c.outcome, Outcome::Refused) {
                        "TRUE"
                    } else {
                        "FALSE"
                    }
                ),
                format!(
                    "nonfinite = {}",
                    if matches!(c.outcome, Outcome::NonFinite) {
                        "TRUE"
                    } else {
                        "FALSE"
                    }
                ),
            ];
            if let Some((value, gradient)) = &c.measured {
                let partials: Vec<f64> = gradient.iter().copied().map(expected_number).collect();
                fields.push(format!(
                    "value = {}",
                    crate::rcode::literal(expected_number(*value))
                ));
                fields.push(format!("gradient = {}", point_r(problem, &partials)));
            }
            format!("  list(\n    {}\n  )", fields.join(",\n    "))
        })
        .collect();
    out.push_str(&entries.join(",\n"));
    out.push_str("\n)\n\nnames(catalyst_fixtures) <- vapply(catalyst_fixtures, function(f) f$name, character(1))\n");
    out
}

/// `test_function.R`, which is the same for every R export: it is driven
/// entirely by `catalyst_fixtures`, so a problem with different parameters
/// needs no different test file.
const TEST_FUNCTION_R: &str = r#"# Generated by `catalyst export r`. Checks the generated function against the
# engine's own measurements, one zero-argument test function per claim.
#
# Run it with the R you already have; no package is loaded:
#
#     Rscript --vanilla -e 'source("test_function.R"); for (t in ls(pattern = "^test_")) get(t)()'
#
# Do not edit: regenerate the export instead.

source("function.R")
source("fixtures.R")

# The tolerance rule the fixtures declare: within the absolute bound, or
# within the relative one of the larger magnitude, whichever is more
# forgiving. `tol_abs` rather than `abs` so the base function stays readable.
catalyst_close <- function(a, b, rel, tol_abs) {
  if (is.nan(a) && is.nan(b)) {
    return(TRUE)
  }
  if (!is.finite(a) || !is.finite(b)) {
    return(a == b)
  }
  abs(a - b) <= max(tol_abs, rel * max(abs(a), abs(b)))
}

test_values_agree_with_the_engine <- function() {
  for (fixture in catalyst_fixtures) {
    if (is.null(fixture$value)) {
      next
    }
    got <- catalyst_value(fixture$inputs)
    if (!catalyst_close(got, fixture$value, catalyst_tolerance$relative,
                        catalyst_tolerance$absolute)) {
      stop(sprintf("case %s: this R computes %.17g where the engine measured %.17g",
                   fixture$name, got, fixture$value))
    }
  }
  invisible(TRUE)
}

test_gradients_agree_with_the_engine <- function() {
  for (fixture in catalyst_fixtures) {
    if (is.null(fixture$gradient)) {
      next
    }
    got <- catalyst_gradient(fixture$inputs)
    for (name in names(fixture$gradient)) {
      if (!catalyst_close(got[[name]], fixture$gradient[[name]],
                          catalyst_tolerance$relative,
                          catalyst_tolerance$absolute)) {
        stop(sprintf("case %s: this R computes d/d%s = %.17g where the engine measured %.17g",
                     fixture$name, name, got[[name]], fixture$gradient[[name]]))
      }
    }
  }
  invisible(TRUE)
}

test_the_domain_rule_is_the_declared_one <- function() {
  for (fixture in catalyst_fixtures) {
    inside <- catalyst_in_domain(fixture$inputs)$ok
    if (inside == isTRUE(fixture$refused)) {
      stop(sprintf("case %s: the domain check says inside = %s, and the fixture says refused = %s",
                   fixture$name, inside, isTRUE(fixture$refused)))
    }
  }
  invisible(TRUE)
}

test_the_cases_the_engine_could_not_measure_stay_unmeasurable <- function() {
  for (fixture in catalyst_fixtures) {
    if (!isTRUE(fixture$nonfinite)) {
      next
    }
    got <- catalyst_value(fixture$inputs)
    if (is.finite(got)) {
      stop(sprintf("case %s: the engine has no number here, and this R returned %.17g",
                   fixture$name, got))
    }
  }
  invisible(TRUE)
}
"#;

fn report_r(problem: &Problem, cases: &[Case]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Validation report: {} (R export)\n", problem.name);
    let _ = writeln!(out, "## The problem\n");
    let _ = writeln!(out, "Goal: {}\n", problem.goal);
    let _ = writeln!(out, "Function:\n");
    let _ = writeln!(out, "    {}\n", problem.function);
    let _ = writeln!(out, "Parameters, in declaration order:\n");
    for (i, name) in problem.params.iter().enumerate() {
        let domain = &problem.domains[i];
        let unit = if domain.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", domain.unit)
        };
        let _ = writeln!(
            out,
            "- `{name}` = {}{unit}, declared inside [{}, {}]",
            json::number(problem.inputs[i]),
            json::number(domain.min),
            json::number(domain.max)
        );
    }
    let _ = writeln!(out, "\n## What was checked\n");
    let _ = writeln!(
        out,
        "`function.R` was generated from the same optimised, differentiated IR the Go\n\
         export is generated from, so the two are two printings of one instruction\n\
         list rather than two implementations. The {} cases below carry the Rust\n\
         engine's own value and gradient at each point, written to {EXPECTED_DIGITS}\n\
         significant digits, and `test_function.R` compares this R against them within\n\
         the tolerance stated next.\n",
        cases.len()
    );
    let _ = writeln!(out, "## Tolerance\n");
    let _ = writeln!(
        out,
        "The R is compared against the Rust engine within relative {} or absolute {},\n\
         whichever is larger. Exact equality is not claimed; see the limitations below.\n",
        json::number(RELATIVE),
        json::number(ABSOLUTE)
    );
    let _ = writeln!(out, "## Cases\n");
    for c in cases {
        let _ = writeln!(out, "### {} ({})\n", c.name, c.kind);
        let at: Vec<String> = problem
            .params
            .iter()
            .zip(&c.inputs)
            .map(|(name, value)| format!("{name} = {}", json::number(*value)))
            .collect();
        let _ = writeln!(out, "At {}.\n", at.join(", "));
        if let Some((value, gradient)) = &c.measured {
            let _ = writeln!(
                out,
                "- engine value: {}",
                json::number(expected_number(*value))
            );
            for (name, g) in problem.params.iter().zip(gradient) {
                let _ = writeln!(
                    out,
                    "- engine d/d{name}: {}",
                    json::number(expected_number(*g))
                );
            }
        }
        match &c.outcome {
            Outcome::Measured { .. } => out.push('\n'),
            Outcome::NonFinite => {
                let _ = writeln!(
                    out,
                    "- the engine has no finite value or partial here, and the export is \
                     expected to report the same.\n"
                );
            }
            Outcome::Refused => {
                let _ = writeln!(
                    out,
                    "- outside the declared domain: `catalyst_in_domain` reports the \
                     violation, and a caller is expected to refuse rather than use the \
                     number above.\n"
                );
            }
        }
    }
    let _ = writeln!(out, "## Running this export\n");
    out.push_str(
        "No package is loaded by any file here, so the R already installed is enough.\n\
         From this directory:\n\n\
         \x20   Rscript --vanilla -e 'source(\"test_function.R\"); for (t in ls(pattern = \"^test_\")) get(t)()'\n\n\
         Silence is a pass: each test stops with the name of the first case that\n\
         disagrees. To use the function rather than check it, `source(\"function.R\")`\n\
         and call `catalyst_value(list(...))` or `catalyst_gradient(list(...))` with a\n\
         named list of parameter values.\n\n",
    );
    let _ = writeln!(out, "## Limitations\n");
    let _ = writeln!(out, "- {LIMITATION_R}");
    let _ = writeln!(out, "- {LIMITATION_FLOAT}");
    let _ = writeln!(out, "\n## Remaining uncertainty\n");
    out.push_str(
        "- The cases above are the points that were measured. Nothing was measured\n\
         \x20 between them, and a function can misbehave between sampled points.\n\
         - The comparison is against this build of the Rust engine, which is a\n\
         \x20 measurement and not a proof: where the engine is wrong, the export\n\
         \x20 reproduces it and the checks still pass.\n\
         - The expected numbers are written to a finite number of digits, and R's\n\
         \x20 elementary functions are the platform's. Agreement is claimed only to the\n\
         \x20 tolerance stated above, on the platform where the check is run.\n\
         - Only the exported function and its gradient were checked. Nothing here says\n\
         \x20 anything about the goal the problem states, or about whether the function\n\
         \x20 is the right model of it.\n",
    );
    out
}

/// Every fixture case, in a fixed order.
fn cases(problem: &Problem) -> Vec<Case> {
    let mut all = Vec::new();
    all.push(case(problem, "inputs", "normal", problem.at().to_vec()));
    for (label, fraction) in [("interior-low", 0.25), ("interior-high", 0.75)] {
        let at: Vec<f64> = problem
            .domains
            .iter()
            .map(|d| d.min + fraction * (d.max - d.min))
            .collect();
        all.push(case(problem, label, "normal", at));
    }
    for (i, name) in problem.params.iter().enumerate() {
        for (edge, bound) in [
            ("min", problem.domains[i].min),
            ("max", problem.domains[i].max),
        ] {
            let mut at = problem.at().to_vec();
            at[i] = bound;
            all.push(case(
                problem,
                &format!("boundary-{name}-{edge}"),
                "boundary",
                at,
            ));
        }
    }
    if let Some(name) = problem.params.first() {
        let at = below_min(problem);
        all.push(Case {
            name: format!("out-of-domain-{name}-below-min"),
            kind: "out_of_domain",
            measured: measure(problem, &at),
            inputs: at,
            outcome: Outcome::Refused,
        });
    }
    all
}

/// A point strictly below the first parameter's declared minimum.
///
/// Below by a visible amount rather than by one bit: a case that only just
/// fails is a case that can pass by rounding on somebody else's platform. But
/// the *strongest* out-of-domain case is one where the engine still has a
/// number, because then an export that dropped its domain check would return
/// that number and be caught, rather than returning `nonfinite` and looking
/// like it refused. So the step shrinks until the engine has an answer, and
/// only falls back to the widest step when it never does.
fn below_min(problem: &Problem) -> Vec<f64> {
    let domain = &problem.domains[0];
    let width = domain.max - domain.min;
    let step = |fraction: f64| {
        let mut at = problem.at().to_vec();
        at[0] = domain.min
            - if width.is_finite() {
                width * fraction
            } else {
                1.0
            };
        at
    };
    for fraction in [0.5, 0.25, 0.1, 0.05, 0.01, 0.001] {
        let at = step(fraction);
        if at[0] < domain.min && measure(problem, &at).is_some() {
            return at;
        }
    }
    step(0.5)
}

/// The engine's value and partials at a point, when all of them are finite.
fn measure(problem: &Problem, at: &[f64]) -> Option<(f64, Vec<f64>)> {
    match crate::api::gradient(&problem.function, at) {
        Ok(answer) if answer.value.is_finite() && answer.gradient.iter().all(|g| g.is_finite()) => {
            Some((answer.value, answer.gradient))
        }
        _ => None,
    }
}

/// The comparison a fixture declares, in one place, so the Rust side, the Go
/// side and `catalyst_close` in the R side are all the same rule: within
/// `abs`, or within `rel` of the larger magnitude, whichever is more
/// forgiving.
pub fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    if a.is_nan() && b.is_nan() {
        return true;
    }
    if !a.is_finite() || !b.is_finite() {
        return a == b;
    }
    (a - b).abs() <= abs.max(rel * a.abs().max(b.abs()))
}

/// One case, measured by running the engine at that point.
fn case(problem: &Problem, name: &str, kind: &'static str, at: Vec<f64>) -> Case {
    let measured = measure(problem, &at);
    let outcome = match &measured {
        Some((value, gradient)) => Outcome::Measured {
            value: *value,
            gradient: gradient.clone(),
        },
        // A point where the engine has no number is documented as one, rather
        // than left out: "the engine is not finite here" is a fact the Go has
        // to reproduce too.
        None => Outcome::NonFinite,
    };
    Case {
        name: name.to_owned(),
        kind,
        inputs: at,
        outcome,
        measured,
    }
}

fn go_mod(problem: &Problem) -> String {
    format!("module catalyst_export_{}\n\ngo 1.21\n", problem.name)
}

fn point(problem: &Problem, at: &[f64]) -> Json {
    Json::Obj(
        problem
            .params
            .iter()
            .zip(at)
            .map(|(name, value)| (name.clone(), Json::Num(*value)))
            .collect(),
    )
}

fn parameters(problem: &Problem) -> Json {
    let domains = Json::Obj(
        problem
            .params
            .iter()
            .zip(&problem.domains)
            .map(|(name, domain)| {
                (
                    name.clone(),
                    obj(vec![
                        ("min", Json::Num(domain.min)),
                        ("max", Json::Num(domain.max)),
                        ("unit", s(&domain.unit)),
                    ]),
                )
            })
            .collect(),
    );
    let units = Json::Obj(
        problem
            .params
            .iter()
            .zip(&problem.domains)
            .map(|(name, domain)| (name.clone(), s(&domain.unit)))
            .collect(),
    );
    obj(vec![
        ("schema", s("catalyst.export-parameters.v1")),
        ("name", s(&problem.name)),
        ("goal", s(&problem.goal)),
        ("function", s(&problem.function)),
        ("inputs", point(problem, problem.at())),
        ("domains", domains),
        ("units", units),
        (
            "limitations",
            Json::Arr(vec![s(LIMITATION_GO), s(LIMITATION_FLOAT)]),
        ),
    ])
}

fn fixtures(problem: &Problem, cases: &[Case]) -> Json {
    let rendered: Vec<Json> = cases
        .iter()
        .map(|c| {
            let expected = match &c.outcome {
                Outcome::Measured { value, gradient } => {
                    let partials: Vec<f64> =
                        gradient.iter().copied().map(expected_number).collect();
                    obj(vec![
                        ("value", Json::Num(expected_number(*value))),
                        ("gradient", point(problem, &partials)),
                    ])
                }
                Outcome::NonFinite => obj(vec![("nonfinite", Json::Bool(true))]),
                Outcome::Refused => obj(vec![("refused", Json::Bool(true))]),
            };
            let mut fields = vec![
                ("name", s(&c.name)),
                ("kind", s(c.kind)),
                ("inputs", point(problem, &c.inputs)),
            ];
            // The engine's own numbers, on every case that has them --
            // including the out-of-domain one, where they are what a
            // conforming export must refuse to return. `expected` below is
            // the contract; these are the measurement.
            if let Some((value, gradient)) = &c.measured {
                let partials: Vec<f64> = gradient.iter().copied().map(expected_number).collect();
                fields.push(("value", Json::Num(expected_number(*value))));
                fields.push(("gradient", point(problem, &partials)));
            }
            fields.push(("expected", expected));
            obj(fields)
        })
        .collect();
    obj(vec![
        ("schema", s("catalyst.fixtures.v1")),
        (
            "tolerance",
            obj(vec![
                ("relative", Json::Num(RELATIVE)),
                ("absolute", Json::Num(ABSOLUTE)),
            ]),
        ),
        ("cases", Json::Arr(rendered)),
    ])
}

fn report(problem: &Problem, cases: &[Case]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Validation report: {}\n", problem.name);
    let _ = writeln!(out, "## The problem\n");
    let _ = writeln!(out, "Goal: {}\n", problem.goal);
    let _ = writeln!(out, "Function:\n");
    let _ = writeln!(out, "    {}\n", problem.function);
    let _ = writeln!(out, "Parameters, in declaration order:\n");
    for (i, name) in problem.params.iter().enumerate() {
        let domain = &problem.domains[i];
        let unit = if domain.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", domain.unit)
        };
        let _ = writeln!(
            out,
            "- `{name}` = {}{unit}, declared inside [{}, {}]",
            json::number(problem.inputs[i]),
            json::number(domain.min),
            json::number(domain.max)
        );
    }
    let _ = writeln!(out, "\n## Tolerance\n");
    let _ = writeln!(
        out,
        "The Go is compared against the Rust engine within relative {} or absolute {},\n\
         whichever is larger. Exact equality is not claimed; see the limitations below.\n",
        json::number(RELATIVE),
        json::number(ABSOLUTE)
    );
    let _ = writeln!(out, "## Cases\n");
    for c in cases {
        let _ = writeln!(out, "### {} ({})\n", c.name, c.kind);
        let at: Vec<String> = problem
            .params
            .iter()
            .zip(&c.inputs)
            .map(|(name, value)| format!("{name} = {}", json::number(*value)))
            .collect();
        let _ = writeln!(out, "At {}.\n", at.join(", "));
        match &c.outcome {
            Outcome::Measured { value, gradient } => {
                let _ = writeln!(
                    out,
                    "- engine value: {}",
                    json::number(expected_number(*value))
                );
                for (name, g) in problem.params.iter().zip(gradient) {
                    let _ = writeln!(
                        out,
                        "- engine d/d{name}: {}",
                        json::number(expected_number(*g))
                    );
                }
                out.push('\n');
            }
            Outcome::NonFinite => {
                let _ = writeln!(
                    out,
                    "- the engine has no finite value or partial here, and the export is \
                     expected to report the same.\n"
                );
            }
            Outcome::Refused => {
                let _ = writeln!(
                    out,
                    "- outside the declared domain: the export is expected to refuse, not \
                     to return a number.\n"
                );
            }
        }
    }
    let _ = writeln!(out, "## Building and running this export\n");
    out.push_str(
        "The module has no requirements and does not use cgo. From this directory:\n\n\
         \x20   go vet ./...\n\
         \x20   go build -o exported-binary .\n\
         \x20   ./exported-binary fixtures.json\n\n\
         The binary prints one JSON array, one object per fixture case: a value and a\n\
         gradient, or `refused` for a point outside the domain, or `nonfinite` where the\n\
         function has no number. Compare each against the `expected` of the same case in\n\
         `fixtures.json`, within the tolerance above.\n\n",
    );
    let _ = writeln!(out, "## Limitations\n");
    let _ = writeln!(out, "- {LIMITATION_GO}");
    let _ = writeln!(out, "- {LIMITATION_FLOAT}");
    out
}

/// `main.go`, which is the same for every export: it reads the fixtures, runs
/// the generated function at each case, and prints one JSON array. Standard
/// library only, and no `unsafe`, `C`, `os/exec`, `net` or `syscall`.
const MAIN_GO: &str = r#"// Generated by `catalyst export go`. Runs the exported function over the
// fixtures and prints one JSON array, one object per case.
//
// Do not edit: regenerate the export instead.

package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
)

type fixtureCase struct {
	Name   string             `json:"name"`
	Kind   string             `json:"kind"`
	Inputs map[string]float64 `json:"inputs"`
}

type fixtureFile struct {
	Cases []fixtureCase `json:"cases"`
}

type caseResult struct {
	Case      string             `json:"case"`
	Value     *float64           `json:"value,omitempty"`
	Gradient  map[string]float64 `json:"gradient,omitempty"`
	Refused   string             `json:"refused,omitempty"`
	NonFinite bool               `json:"nonfinite,omitempty"`
}

func fail(what string) {
	fmt.Fprintln(os.Stderr, what)
	os.Exit(2)
}

func finite(x float64) bool {
	return !math.IsNaN(x) && !math.IsInf(x, 0)
}

func main() {
	if len(os.Args) != 2 {
		fail("usage: exported-binary FIXTURES")
	}
	raw, err := os.ReadFile(os.Args[1])
	if err != nil {
		fail("the fixtures file could not be read")
	}
	var fixtures fixtureFile
	if err := json.Unmarshal(raw, &fixtures); err != nil {
		fail("the fixtures file is not the expected JSON")
	}

	results := make([]caseResult, 0, len(fixtures.Cases))
	for _, one := range fixtures.Cases {
		in := make([]float64, len(InputNames))
		for i, name := range InputNames {
			value, ok := one.Inputs[name]
			if !ok {
				fail("case " + one.Name + " has no input for " + name)
			}
			in[i] = value
		}
		if ok, why := InDomain(in); !ok {
			results = append(results, caseResult{Case: one.Name, Refused: "out of domain: " + why})
			continue
		}
		value, gradient := Gradient(in)
		ok := finite(value)
		for _, g := range gradient {
			if !finite(g) {
				ok = false
			}
		}
		if !ok {
			results = append(results, caseResult{Case: one.Name, NonFinite: true})
			continue
		}
		partials := make(map[string]float64, len(InputNames))
		for i, name := range InputNames {
			partials[name] = gradient[i]
		}
		measured := value
		results = append(results, caseResult{Case: one.Name, Value: &measured, Gradient: partials})
	}

	encoded, err := json.Marshal(results)
	if err != nil {
		fail("the results could not be encoded")
	}
	fmt.Println(string(encoded))
}
"#;
