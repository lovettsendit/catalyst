//! The derivative artifact: `catalyst.derivative.v1`.
//!
//! `docs/interface.md` §12.4. An artifact is six files (seven with rules),
//! and the document among them is a chain of digests: the source, the
//! compiler's IR, the primal in Catalyst's IR, the derivative, the
//! validation. Each link is the SHA-256 of a file that sits beside the
//! document, so a reader can recompute every one, and `catalyst
//! differentiate run` does.
//!
//! # What is in it and what is not
//!
//! Every byte is a function of the request: the module, the function, the
//! inputs, the point, the mode, the rules. No path, no clock, no environment
//! value, no host name. The compiler's version string from `!llvm.ident` is
//! copied verbatim, because it is provenance and it is the same on every run
//! over the same module. Two runs of one request are the same bytes, which is
//! what makes an artifact reviewable in a diff and what lets `run` check it
//! by regenerating rather than by trusting.
//!
//! # `validated:false` is never written
//!
//! The artifact type carries its validation as an `Option`, and the only
//! function that turns it into files takes the cases by value from a
//! successful verification. There is no other way to produce the files.

use super::digest::sha256;
use super::engine::{Derivative, Mode};
use super::provenance::{cgc_version, Provenance};
use super::verify::{self, Case};
use crate::interp::Fault;
use crate::ir::Ty;
use crate::json::{self, obj, s, Json};
use std::fmt::Write as _;

pub const SCHEMA: &str = "catalyst.derivative.v1";
pub const FIXTURES_SCHEMA: &str = "catalyst.derivative-fixtures.v1";

/// The sentence every report carries, verbatim.
pub const SENTENCE: &str = "The derivative was produced by the Catalyst Gradient Compiler and \
                            checked by Catalyst's own verifier; neither one declares the other \
                            correct.";

/// Which lane produced the artifact, and what that lane can promise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Textual LLVM IR through the Catalyst Gradient Compiler.
    Llvm,
    /// The expression language through the native backend.
    Expression,
}

impl Lane {
    pub fn backend(self) -> &'static str {
        match self {
            Lane::Llvm => "cgc",
            Lane::Expression => "native",
        }
    }
    pub fn computation_kind(self) -> &'static str {
        match self {
            Lane::Llvm => "llvm",
            Lane::Expression => "catalyst-ir",
        }
    }
    pub fn portable_export(self) -> bool {
        self == Lane::Expression
    }
    /// The file the input is kept in, byte for byte.
    pub fn input_file(self) -> &'static str {
        match self {
            Lane::Llvm => "module.ll",
            Lane::Expression => "problem.json",
        }
    }
    /// The `assurance` block, which is the same on every answer of the lane.
    pub fn assurance(self, validated: bool) -> Json {
        obj(vec![
            ("computation_kind", s(self.computation_kind())),
            ("differentiation_backend", s(self.backend())),
            ("validated", Json::Bool(validated)),
            ("portable_export", Json::Bool(self.portable_export())),
        ])
    }
}

/// A derivative, with everything the artifact says about it.
pub struct DerivativeArtifact {
    pub lane: Lane,
    pub function: String,
    /// The requested inputs, in request order.
    pub inputs: Vec<String>,
    /// Every parameter, in declaration order.
    pub parameters: Vec<(String, Ty)>,
    /// The declared point, over every parameter.
    pub at: Vec<f64>,
    pub derivative: Derivative,
    /// The input, byte for byte: the module, or the validated problem.
    pub input_text: String,
    pub primal_cir: String,
    pub derivative_cir: String,
    pub provenance: Provenance,
    pub rules_applied: Vec<String>,
    /// The rules document as read, when one was given.
    pub rules_text: Option<String>,
    /// `--source`: its base name and its digest.
    pub source: Option<(String, String)>,
    /// The verifier's cases, once it has run. `None` is an artifact nothing
    /// may write.
    pub validation: Option<Vec<Case>>,
}

/// The `validation` object of an answer.
pub fn validation_json(points: usize) -> Json {
    obj(vec![
        ("points", Json::Num(points as f64)),
        ("passed", Json::Num(points as f64)),
        ("relative_tolerance", Json::Num(verify::RELATIVE)),
        ("absolute_tolerance", Json::Num(verify::ABSOLUTE)),
    ])
}

fn number_or_null(x: f64) -> Json {
    if x.is_finite() {
        Json::Num(x)
    } else {
        Json::Null
    }
}

impl DerivativeArtifact {
    pub fn mode(&self) -> Mode {
        self.derivative.mode
    }

    /// The value and the requested partials at the declared point, from the
    /// derivative function. Computed when asked for rather than stored, so
    /// that nothing is evaluated before a caller has decided to.
    pub fn declared(&self) -> Result<(f64, Vec<f64>), Fault> {
        self.derivative.evaluate(&self.at)
    }

    /// `at` as an object over every parameter.
    pub fn at_json(&self) -> Json {
        Json::Obj(
            self.parameters
                .iter()
                .zip(&self.at)
                .map(|((name, _), &x)| (name.clone(), number_or_null(x)))
                .collect(),
        )
    }

    /// The members every answer shares, after `ok` and whatever names the
    /// command: backend, mode, function, inputs, at, value, gradient, finite.
    pub fn answer_members(&self, value: f64, gradient: &[f64]) -> Vec<(&'static str, Json)> {
        let finite = value.is_finite() && gradient.iter().all(|g| g.is_finite());
        vec![
            ("backend", s(self.lane.backend())),
            ("mode", s(self.mode().name())),
            ("function", s(&self.function)),
            (
                "inputs",
                Json::Arr(self.inputs.iter().map(|i| s(i)).collect()),
            ),
            ("at", self.at_json()),
            ("value", number_or_null(value)),
            ("gradient", gradient_object(&self.inputs, gradient)),
            ("finite", Json::Bool(finite)),
        ]
    }

    /// The files, in the order the answer lists them, with their contents.
    /// Only a validated artifact has files.
    pub fn files(&self) -> Option<Vec<(&'static str, String)>> {
        let cases = self.validation.as_ref()?;
        let fixtures = self.fixtures(cases).render_pretty();
        // The document is one line, in the compact spelling every answer
        // uses: it is the machine-read half of the artifact, and the report
        // beside it is the half a person reads.
        let mut document = self.document(&fixtures, cases.len()).render();
        document.push('\n');
        let mut files = vec![
            ("derivative.json", document),
            (self.lane.input_file(), self.input_text.clone()),
            ("primal.cir", self.primal_cir.clone()),
            ("derivative.cir", self.derivative_cir.clone()),
            ("fixtures.json", fixtures),
            ("validation-report.md", self.report(cases)),
        ];
        if let Some(rules) = &self.rules_text {
            files.push(("rules.json", rules.clone()));
        }
        Some(files)
    }

    fn fixtures(&self, cases: &[Case]) -> Json {
        obj(vec![
            ("schema", s(FIXTURES_SCHEMA)),
            ("function", s(&self.function)),
            (
                "inputs",
                Json::Arr(self.inputs.iter().map(|i| s(i)).collect()),
            ),
            (
                "tolerance",
                obj(vec![
                    ("relative", Json::Num(verify::RELATIVE)),
                    ("absolute", Json::Num(verify::ABSOLUTE)),
                ]),
            ),
            (
                "cases",
                Json::Arr(
                    cases
                        .iter()
                        .map(|case| {
                            obj(vec![
                                ("case", s(&case.name)),
                                (
                                    "at",
                                    Json::Obj(
                                        self.parameters
                                            .iter()
                                            .zip(&case.at)
                                            .map(|((name, _), &x)| {
                                                (name.clone(), number_or_null(x))
                                            })
                                            .collect(),
                                    ),
                                ),
                                ("value", number_or_null(case.value)),
                                ("gradient", gradient_object(&self.inputs, &case.gradient)),
                                ("estimate", gradient_object(&self.inputs, &case.estimate)),
                                ("agrees", Json::Bool(case.agrees)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// `derivative.json`.
    fn document(&self, fixtures: &str, points: usize) -> Json {
        let (source_digest, source_name) = match &self.source {
            Some((name, digest)) if name.is_empty() => (s(digest), Json::Null),
            Some((name, digest)) => (s(digest), s(name)),
            None => (Json::Null, Json::Null),
        };
        // The expression lane's "compiler IR" is the primal: Catalyst is the
        // compiler, and the primal is what it wrote.
        let compiler_ir = match self.lane {
            Lane::Llvm => sha256(self.input_text.as_bytes()),
            Lane::Expression => sha256(self.primal_cir.as_bytes()),
        };
        obj(vec![
            ("schema", s(SCHEMA)),
            ("backend", s(self.lane.backend())),
            ("mode", s(self.mode().name())),
            ("function", s(&self.function)),
            (
                "inputs",
                Json::Arr(self.inputs.iter().map(|i| s(i)).collect()),
            ),
            ("outputs", Json::Arr(vec![s(&self.function)])),
            (
                "parameters",
                Json::Arr(
                    self.parameters
                        .iter()
                        .map(|(name, ty)| {
                            obj(vec![
                                ("name", s(name)),
                                (
                                    "type",
                                    s(match ty {
                                        Ty::Real => "double",
                                        _ => "i64",
                                    }),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("source_digest", source_digest),
            ("source_name", source_name),
            ("compiler_ir_digest", s(&compiler_ir)),
            ("primal_ir_digest", s(&sha256(self.primal_cir.as_bytes()))),
            (
                "derivative_digest",
                s(&sha256(self.derivative_cir.as_bytes())),
            ),
            ("validation_digest", s(&sha256(fixtures.as_bytes()))),
            (
                "rules_applied",
                Json::Arr(self.rules_applied.iter().map(|r| s(r)).collect()),
            ),
            (
                "provenance",
                obj(vec![
                    ("compiler", s(&self.provenance.compiler)),
                    ("target_triple", s(&self.provenance.target_triple)),
                    ("cgc_version", s(cgc_version())),
                    ("mode", s(self.mode().name())),
                    (
                        "inputs",
                        Json::Arr(self.inputs.iter().map(|i| s(i)).collect()),
                    ),
                    ("output", s(&self.function)),
                ]),
            ),
            ("assurance", self.lane.assurance(true)),
            ("gradient_validation", validation_json(points)),
        ])
    }

    /// `validation-report.md`.
    fn report(&self, cases: &[Case]) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Derivative of `{}`", self.function);
        out.push('\n');
        let _ = writeln!(
            out,
            "Backend: `{}`. Mode: `{}`. Inputs: {}.",
            self.lane.backend(),
            self.mode().name(),
            self.inputs
                .iter()
                .map(|i| format!("`{i}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        out.push('\n');
        let _ = writeln!(out, "{SENTENCE}");
        out.push('\n');

        out.push_str("## Validation\n\n");
        let _ = writeln!(
            out,
            "Catalyst's verifier evaluated the primal -- the function as lowered and \
             optimised, with the derivative transform nowhere in the path -- and formed \
             central finite-difference estimates of every requested partial at {} points: \
             the declared point, and each input moved one per cent up and down. Every \
             estimate agreed with the derivative within relative {} or absolute {}.",
            cases.len(),
            json::number(verify::RELATIVE),
            json::number(verify::ABSOLUTE)
        );
        out.push('\n');
        for case in cases {
            let _ = writeln!(out, "### `{}`", case.name);
            out.push('\n');
            let at: Vec<String> = self
                .parameters
                .iter()
                .zip(&case.at)
                .map(|((name, _), &x)| format!("{name} = {}", json::number(x)))
                .collect();
            let _ = writeln!(
                out,
                "At {}: value {}.",
                at.join(", "),
                json::number(case.value)
            );
            out.push('\n');
            out.push_str("| input | derivative | verifier's estimate | agrees |\n");
            out.push_str("| --- | --- | --- | --- |\n");
            for ((name, &g), &e) in self.inputs.iter().zip(&case.gradient).zip(&case.estimate) {
                let _ = writeln!(
                    out,
                    "| `{name}` | {} | {} | {} |",
                    json::number(g),
                    json::number(e),
                    if case.agrees { "yes" } else { "no" }
                );
            }
            out.push('\n');
        }

        out.push_str("## Provenance\n\n");
        let _ = writeln!(
            out,
            "- compiler: `{}`\n- target: `{}`\n- Catalyst Gradient Compiler: `{}`",
            self.provenance.compiler,
            self.provenance.target_triple,
            cgc_version()
        );
        match &self.source {
            Some((name, digest)) if name.is_empty() => {
                let _ = writeln!(out, "- source (the function text): sha256 `{digest}`");
            }
            Some((name, digest)) => {
                let _ = writeln!(out, "- source `{name}`: sha256 `{digest}`");
            }
            None => out.push_str("- source: not given\n"),
        }
        let _ = writeln!(
            out,
            "- `{}`: sha256 `{}`",
            self.lane.input_file(),
            sha256(self.input_text.as_bytes())
        );
        let _ = writeln!(
            out,
            "- `primal.cir`: sha256 `{}`",
            sha256(self.primal_cir.as_bytes())
        );
        let _ = writeln!(
            out,
            "- `derivative.cir`: sha256 `{}`",
            sha256(self.derivative_cir.as_bytes())
        );
        if self.rules_applied.is_empty() {
            out.push_str("- rules applied: none\n");
        } else {
            let _ = writeln!(
                out,
                "- rules applied: {}",
                self.rules_applied
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        out.push('\n');

        out.push_str("## Remaining uncertainty\n\n");
        out.push_str(
            "- The verifier checked the derivative at the points above and nowhere else. \
             A function whose derivative is right here and wrong elsewhere has not been \
             ruled out; more points would narrow that, not close it.\n",
        );
        out.push_str(
            "- A finite-difference estimate is itself an approximation, accurate to about \
             1e-10 relative for a smooth function at this step. Agreement within 1e-6 is \
             therefore evidence, not proof, and a disagreement below that level would not \
             have been seen.\n",
        );
        out.push_str(
            "- The program was executed only by Catalyst's interpreter. Its behaviour under \
             the compiler's own code generation -- a fused multiply-add, a different \
             evaluation order -- may differ in the last bits.\n",
        );
        if !self.rules_applied.is_empty() {
            out.push_str(
                "- A custom rule was applied. It was checked at the points above exactly as \
                 a generated derivative is, and it is trusted no further than that.\n",
            );
        }
        match self.lane {
            Lane::Llvm => out.push_str(
                "- There is no portable export of this derivative: it exists as Catalyst's \
                 IR and as this artifact, and nowhere else.\n",
            ),
            Lane::Expression => out.push_str(
                "- The expression lane's Go and R exports are checked separately by their \
                 own fixtures; this artifact says nothing about them.\n",
            ),
        }
        out
    }
}

fn gradient_object(names: &[String], values: &[f64]) -> Json {
    Json::Obj(
        names
            .iter()
            .zip(values)
            .map(|(name, &g)| (name.clone(), number_or_null(g)))
            .collect(),
    )
}
