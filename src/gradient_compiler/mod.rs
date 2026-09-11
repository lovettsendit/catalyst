//! The Catalyst Gradient Compiler.
//!
//! `docs/interface.md` §12. Catalyst's engine differentiates SSA programs:
//! blocks, branches, phis, integer counters, selects. The expression language
//! of §1 is one front end onto it. This subsystem is the second: it reads the
//! textual LLVM IR a user's own compiler wrote, lowers it to the engine's IR,
//! differentiates it with the transforms that already exist, and hands back a
//! **derivative artifact** that Catalyst's verifier checks against central
//! finite differences before a byte of it is written. The compiler proposes;
//! the verifier decides; neither declares the other correct.
//!
//! # The shape
//!
//! ```text
//!   Program ──► DifferentiationBackend::differentiate ──► DerivativeArtifact
//!                     │ native: the expression language              │
//!                     │ cgc:    textual LLVM IR                       │
//!                                                                     ▼
//!                                              verify::validate, over the primal
//!                                                                     │
//!                                                      files, or a refusal and nothing
//! ```
//!
//! [`DifferentiationBackend`] is the seam: a backend takes a program and a
//! request and produces an artifact whose derivative is *proposed*. The
//! verifier is not a backend and is not behind the seam; it is Catalyst's
//! own, and [`differentiate`] runs it over every artifact from every backend
//! before anything leaves this module validated.
//!
//! # What this directory never does
//!
//! It runs no process, opens no socket, reads no environment variable and
//! touches no file. The command layer reads the module and writes the
//! artifact through [`crate::paths`], as every command does; this module
//! takes text and returns text, and the only thing that executes the
//! program under differentiation is the engine's interpreter, under a fuel
//! budget. Oracle 12.8 scans this directory for the four ways out and finds
//! none.

pub mod artifact;
pub mod digest;
pub mod engine;
pub mod llvm;
pub mod lower;
pub mod provenance;
pub mod rules;
pub mod verify;

use crate::cli::Refused;
use crate::interp::Fault;
use crate::ir::{self, print_func, Ty};
use crate::json::{self, obj, s, Json};
use crate::opt::{optimise, Level};
use crate::problem::Problem;
use artifact::{DerivativeArtifact, Lane};
use engine::Mode;
use lower::{LowerError, RuleBinding};
use provenance::Provenance;
use std::collections::HashMap;

pub const DESCRIBE_SCHEMA: &str = "catalyst.differentiation.v1";
/// The largest module the reader accepts.
pub const MAX_MODULE_BYTES: usize = 4 * 1024 * 1024;

/// What is to be differentiated.
pub enum Program {
    /// Textual LLVM IR, as read.
    LlvmIr(String),
    /// A validated problem of the expression language.
    CatalystIr(Problem),
}

/// A rules document: the text, kept for the artifact, and what it binds.
#[derive(Clone, Debug)]
pub struct Rules {
    pub text: String,
    pub bindings: Vec<RuleBinding>,
}

/// What is asked of a backend.
#[derive(Clone, Debug)]
pub struct Request {
    /// The function to differentiate, by name.
    pub function: String,
    /// The parameters to differentiate with respect to, in the order the
    /// gradient is to be reported.
    pub inputs: Vec<String>,
    /// A value for every parameter, in declaration order.
    pub at: Vec<f64>,
    /// A forced mode, or `None` to let the backend choose.
    pub mode: Option<Mode>,
    pub rules: Option<Rules>,
    /// `--source`: the base name and the digest of the original program.
    pub source: Option<(String, String)>,
}

/// One way of producing a derivative. The verifier is not one of these.
pub trait DifferentiationBackend {
    fn name(&self) -> &'static str;
    /// Produce a derivative for `program` as `request` asks. The artifact
    /// comes back *unvalidated*: [`differentiate`] runs the verifier over it.
    fn differentiate(
        &self,
        program: &Program,
        request: &Request,
    ) -> Result<DerivativeArtifact, Refused>;
}

/// The expression lane: `catalyst-ir` in, reverse mode.
pub struct Native;
/// The Catalyst Gradient Compiler: `llvm-ir` in, either mode.
pub struct Cgc;

/// The backend for a program: there is exactly one per kind.
pub fn backend_for(program: &Program) -> &'static dyn DifferentiationBackend {
    match program {
        Program::LlvmIr(_) => &Cgc,
        Program::CatalystIr(_) => &Native,
    }
}

/// `catalyst differentiate --describe`.
pub fn describe() -> Json {
    let backend = |name: &str, input: &str, modes: &[&str]| {
        obj(vec![
            ("name", s(name)),
            ("input", s(input)),
            ("modes", Json::Arr(modes.iter().map(|m| s(m)).collect())),
        ])
    };
    obj(vec![
        ("schema", s(DESCRIBE_SCHEMA)),
        (
            "backends",
            Json::Arr(vec![
                backend("native", "catalyst-ir", &["reverse"]),
                backend("cgc", "llvm-ir", &["forward", "reverse"]),
            ]),
        ),
        ("artifact", s(artifact::SCHEMA)),
        (
            "validation",
            obj(vec![
                ("method", s("central finite differences over the primal")),
                ("relative_tolerance", Json::Num(verify::RELATIVE)),
                ("absolute_tolerance", Json::Num(verify::ABSOLUTE)),
            ]),
        ),
    ])
}

/// A derivative, validated: the artifact, and the value and gradient at the
/// declared point.
pub struct Validated {
    pub artifact: DerivativeArtifact,
    pub value: f64,
    pub gradient: Vec<f64>,
}

impl Validated {
    pub fn points(&self) -> usize {
        self.artifact.validation.as_ref().map_or(0, Vec::len)
    }
    /// The `validation` member of an answer.
    pub fn validation_json(&self) -> Json {
        artifact::validation_json(self.points())
    }
    /// The `assurance` member of an answer.
    pub fn assurance_json(&self) -> Json {
        self.artifact.lane.assurance(true)
    }
    /// The `cost` member, in the shape `catalyst eval` prints.
    pub fn cost_json(&self) -> Json {
        obj(vec![
            (
                "primal_insts",
                Json::Num(self.artifact.derivative.primal_insts as f64),
            ),
            (
                "adjoint_insts",
                Json::Num(self.artifact.derivative.derived_insts as f64),
            ),
        ])
    }
}

/// Differentiate and verify. This is the only way to a validated artifact:
/// the backend proposes, the verifier checks every requested partial at the
/// declared point and around it, and a disagreement is a refusal with both
/// numbers in it.
pub fn differentiate(program: &Program, request: &Request) -> Result<Validated, Refused> {
    let mut artifact = backend_for(program).differentiate(program, request)?;
    let cases = verify::validate(&artifact.derivative, &artifact.at, &artifact.inputs).map_err(
        |verdict| match verdict {
            verify::Verdict::Disagrees {
                input,
                point,
                derivative,
                estimate,
                case,
            } => {
                let at: Vec<String> = artifact
                    .parameters
                    .iter()
                    .zip(&point)
                    .map(|((name, _), &x)| format!("{name} = {}", json::number(x)))
                    .collect();
                Refused::new(
                    "catalyst.derivative_disagrees",
                    format!(
                        "the derivative with respect to `{input}` at the point {} ({case}) is \
                         {}, and the verifier's central-difference estimate over the primal \
                         is {}; they disagree beyond relative {} or absolute {}",
                        at.join(", "),
                        json::number(derivative),
                        json::number(estimate),
                        json::number(verify::RELATIVE),
                        json::number(verify::ABSOLUTE)
                    ),
                    "the derivative is not trusted and nothing was written; if a custom rule \
                     applied, check its derivative function against the function it claims to \
                     differentiate, otherwise report the module and the point",
                )
            }
            verify::Verdict::Faulted(fault) => faulted(&artifact.function, &fault),
        },
    )?;
    // The declared point is the first case, so the answer's numbers are the
    // verifier's own numbers for it.
    let (value, gradient) = match cases.first() {
        Some(case) => (case.value, case.gradient.clone()),
        None => artifact
            .declared()
            .map_err(|fault| faulted(&artifact.function, &fault))?,
    };
    artifact.validation = Some(cases);
    Ok(Validated {
        artifact,
        value,
        gradient,
    })
}

/// The refusal for a program that faulted under the interpreter.
pub fn faulted(function: &str, fault: &Fault) -> Refused {
    Refused::new(
        "catalyst.llvm_run_faulted",
        format!("running `{function}` under the engine's interpreter faulted: {fault}"),
        "the program was executed only inside Catalyst's interpreter under a budget of 2^26 \
         instructions per evaluation; check that the function terminates for these inputs \
         and reads no memory",
    )
}

// ---------------------------------------------------------------------------
// the native backend
// ---------------------------------------------------------------------------

impl DifferentiationBackend for Native {
    fn name(&self) -> &'static str {
        "native"
    }

    fn differentiate(
        &self,
        program: &Program,
        request: &Request,
    ) -> Result<DerivativeArtifact, Refused> {
        let Program::CatalystIr(problem) = program else {
            return Err(Refused::usage(
                "the native backend differentiates the expression language; an LLVM module \
                 goes to the cgc backend",
            ));
        };
        if request.mode == Some(Mode::Forward) {
            return Err(Refused::usage(
                "the native backend has one mode, reverse; leave --mode out or say reverse",
            ));
        }
        if request.rules.is_some() {
            return Err(Refused::usage(
                "custom rules apply to an LLVM module; the expression language has no calls \
                 to bind them to",
            ));
        }
        let mut parsed = crate::text::parse(&problem.function).map_err(Refused::from)?;
        let primal = parsed.id;
        optimise(&mut parsed.module, primal, Level::Full);
        let primal_cir = print_func(&parsed.module, primal);
        let active: Vec<usize> = (0..parsed.params.len()).collect();
        let derivative = engine::differentiate(
            parsed.module,
            primal,
            active,
            &HashMap::new(),
            Some(Mode::Reverse),
        )
        .map_err(Refused::from)?;
        let derivative_cir = print_func(&derivative.module, derivative.derivative);
        let function_digest = digest::sha256(problem.function.as_bytes());
        Ok(DerivativeArtifact {
            lane: Lane::Expression,
            function: parsed.name.clone(),
            inputs: parsed.params.clone(),
            parameters: parsed
                .params
                .iter()
                .map(|name| (name.clone(), Ty::Real))
                .collect(),
            at: problem.at().to_vec(),
            derivative,
            input_text: problem.to_json().render_pretty(),
            primal_cir,
            derivative_cir,
            provenance: Provenance::native(),
            rules_applied: Vec::new(),
            rules_text: None,
            // The source of the expression lane is the function text.
            source: Some((String::new(), function_digest)),
            validation: None,
        })
    }
}

// ---------------------------------------------------------------------------
// the Catalyst Gradient Compiler
// ---------------------------------------------------------------------------

/// A module as read, the requested function lowered out of it, and that
/// function's parameters by name and engine type.
type ReadAndLowered = (llvm::Module, lower::Lowered, Vec<(String, Ty)>);

/// The read module and the function in it, checked to the subset and
/// lowered: what both `differentiate llvm` and `run` need before anything
/// else.
fn lowered_module(text: &str, request: &Request) -> Result<ReadAndLowered, Refused> {
    if text.len() > MAX_MODULE_BYTES {
        return Err(Refused::new(
            "catalyst.llvm_module_too_long",
            format!(
                "the module is {} bytes, over the {MAX_MODULE_BYTES} byte limit",
                text.len()
            ),
            "differentiate a module holding only the function in question and what it calls",
        ));
    }
    let module = llvm::parse(text).map_err(|error| {
        Refused::new(
            "catalyst.llvm_syntax",
            format!("line {}: {}", error.line, error.what),
            "hand Catalyst the textual LLVM IR a compiler wrote: `rustc --emit=llvm-ir` or \
             `clang -S -emit-llvm`; bitcode is not read",
        )
    })?;
    let Some(def) = module.define(&request.function) else {
        let defined: Vec<String> = module.defines.iter().map(|f| f.name.clone()).collect();
        return Err(Refused::new(
            "catalyst.llvm_function_not_found",
            format!(
                "the module does not define `@{}`; it defines: {}",
                safe(&request.function),
                if defined.is_empty() {
                    "nothing".to_owned()
                } else {
                    defined.join(", ")
                }
            ),
            "name one of the functions the module defines, without the `@`",
        ));
    };
    let bindings: Vec<RuleBinding> = request
        .rules
        .as_ref()
        .map(|r| r.bindings.clone())
        .unwrap_or_default();
    rules::check(&bindings, &module)?;
    let lowered = lower::lower(&module, &def.name, &bindings).map_err(|error| match error {
        LowerError::Unsupported { what, line } => Refused::new(
            "catalyst.llvm_unsupported",
            format!("line {line}: {what} is outside the subset this phase differentiates"),
            "the subset is scalar double and integer arithmetic, comparisons, select, \
             branches, phis, loops, the math intrinsics and calls to functions the module \
             defines; memory, aggregates, vectors and other float widths are not read",
        ),
        LowerError::OpaqueCall { callee, line } => Refused::new(
            "catalyst.opaque_call",
            format!(
                "line {line}: the call to `@{callee}` cannot be seen through, and the module \
                 was not executed"
            ),
            "define the callee in the module so that it can be differentiated through, or \
             register a custom rule for it with --rules",
        ),
        LowerError::Malformed { what, line } => Refused::new(
            "catalyst.llvm_syntax",
            format!("line {line}: {what}"),
            "hand Catalyst the textual LLVM IR a compiler wrote, unedited",
        ),
    })?;
    let parameters: Vec<(String, Ty)> = def
        .params
        .iter()
        .zip(&lowered.module.get(lowered.id).params)
        .map(|(p, &ty)| (p.name.clone(), ty))
        .collect();
    Ok((module, lowered, parameters))
}

/// The requested inputs as parameter indices, in request order.
fn active_indices(parameters: &[(String, Ty)], inputs: &[String]) -> Result<Vec<usize>, Refused> {
    if inputs.is_empty() {
        return Err(Refused::usage(
            "--inputs names at least one parameter to differentiate with respect to",
        ));
    }
    let mut active = Vec::new();
    for name in inputs {
        let Some(index) = parameters.iter().position(|(p, _)| p == name) else {
            return Err(Refused::new(
                "catalyst.unknown_name",
                format!(
                    "`{}` is not a parameter of the function; its parameters are {}",
                    safe(name),
                    parameters
                        .iter()
                        .map(|(p, _)| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "name the parameters by their IR names without the `%`",
            ));
        };
        if parameters[index].1 != Ty::Real {
            return Err(Refused::new(
                "catalyst.llvm_input_not_real",
                format!(
                    "`{}` is an integer parameter, and a derivative is taken only with respect \
                     to a double",
                    safe(name)
                ),
                "name double parameters in --inputs; an integer parameter is held fixed at \
                 the value --at gives it",
            ));
        }
        if active.contains(&index) {
            return Err(Refused::usage(format!(
                "`{}` is named twice in --inputs",
                safe(name)
            )));
        }
        active.push(index);
    }
    Ok(active)
}

/// The declared point, checked against the parameters.
pub fn check_point(function: &str, parameters: &[(String, Ty)], at: &[f64]) -> Result<(), Refused> {
    if at.len() != parameters.len() {
        return Err(Refused::new(
            "catalyst.wrong_arity",
            format!(
                "`{}` has {} parameter(s) and --at gives {} value(s)",
                safe(function),
                parameters.len(),
                at.len()
            ),
            "give --at one value per parameter, in declaration order, comma separated",
        ));
    }
    for ((name, ty), &x) in parameters.iter().zip(at) {
        if !x.is_finite() {
            return Err(Refused::new(
                "catalyst.llvm_argument_invalid",
                format!("the value for `{}` is not finite", safe(name)),
                "give every parameter a finite value",
            ));
        }
        if *ty == Ty::Int && (x.fract() != 0.0 || x.abs() > 9.007_199_254_740_992e15) {
            return Err(Refused::new(
                "catalyst.llvm_argument_invalid",
                format!(
                    "`{}` is an integer parameter and its value is not a whole number the \
                     integer can hold",
                    safe(name)
                ),
                "give an integer parameter a whole number",
            ));
        }
    }
    Ok(())
}

impl DifferentiationBackend for Cgc {
    fn name(&self) -> &'static str {
        "cgc"
    }

    fn differentiate(
        &self,
        program: &Program,
        request: &Request,
    ) -> Result<DerivativeArtifact, Refused> {
        let Program::LlvmIr(text) = program else {
            return Err(Refused::usage(
                "the cgc backend differentiates textual LLVM IR; a problem of the expression \
                 language goes to the native backend",
            ));
        };
        let (module, lowered, parameters) = lowered_module(text, request)?;
        let active = active_indices(&parameters, &request.inputs)?;
        check_point(&request.function, &parameters, &request.at)?;

        let mut ir_module = lowered.module;
        let primal = lowered.id;
        optimise(&mut ir_module, primal, Level::Full);
        let primal_cir = print_func(&ir_module, primal);
        let rule_ids: HashMap<ir::FuncId, ir::FuncId> = lowered
            .rules_applied
            .iter()
            .map(|(_, f, g)| (*f, *g))
            .collect();
        let derivative = engine::differentiate(ir_module, primal, active, &rule_ids, request.mode)
            .map_err(Refused::from)?;
        let derivative_cir = print_func(&derivative.module, derivative.derivative);
        Ok(DerivativeArtifact {
            lane: Lane::Llvm,
            function: request.function.clone(),
            inputs: request.inputs.clone(),
            parameters,
            at: request.at.clone(),
            derivative,
            input_text: text.clone(),
            primal_cir,
            derivative_cir,
            provenance: Provenance::of_module(&module),
            rules_applied: lowered
                .rules_applied
                .iter()
                .map(|(name, _, _)| name.clone())
                .collect(),
            rules_text: request.rules.as_ref().map(|r| r.text.clone()),
            source: request.source.clone(),
            validation: None,
        })
    }
}

/// The parameters of a function in a module, for a caller that needs them
/// before it has a request: the problem reader, matching `inputs` and
/// `fixed` against the function.
pub fn parameters_of(text: &str, function: &str) -> Result<Vec<(String, Ty)>, Refused> {
    let request = Request {
        function: function.to_owned(),
        inputs: Vec::new(),
        at: Vec::new(),
        mode: None,
        rules: None,
        source: None,
    };
    lowered_module(text, &request).map(|(_, _, parameters)| parameters)
}

// ---------------------------------------------------------------------------
// running an artifact again
// ---------------------------------------------------------------------------

/// What `catalyst differentiate run` reads from an artifact directory.
pub struct StoredArtifact {
    pub document: String,
    /// `module.ll` or `problem.json`, whichever the document's backend keeps.
    pub input: String,
    pub rules: Option<String>,
    /// The listings a person reads -- `primal.cir`, `derivative.cir` and
    /// `fixtures.json` -- as found on disk. A rerun checks each against what
    /// it regenerates or recorded and refuses if any differs, so what a
    /// reader was shown is what ran.
    pub listings: Vec<(String, String)>,
}

fn tampered(which: &str, detail: String) -> Refused {
    Refused::new(
        "catalyst.artifact_tampered",
        format!("{which} does not match: {detail}"),
        "the artifact was changed after it was written; regenerate it with `catalyst \
         differentiate` rather than editing it",
    )
}

/// Execute an artifact again at `at`: check `compiler_ir_digest` over the
/// input, regenerate the derivative in the recorded mode with the recorded
/// rules, check `derivative_digest` over it, and only then evaluate.
pub fn rerun(
    stored: &StoredArtifact,
    at: &[f64],
) -> Result<(DerivativeArtifact, f64, Vec<f64>), Refused> {
    let document = Json::parse(&stored.document).map_err(|error| {
        tampered(
            "derivative.json",
            format!(
                "it is not JSON: at byte {}, expected {}",
                error.at, error.what
            ),
        )
    })?;
    let field = |key: &str| -> Result<String, Refused> {
        document
            .get(key)
            .and_then(Json::as_str)
            .map(str::to_owned)
            .ok_or_else(|| tampered("derivative.json", format!("it has no string `{key}`")))
    };
    if field("schema")? != artifact::SCHEMA {
        return Err(tampered(
            "derivative.json",
            format!("its schema is not {}", artifact::SCHEMA),
        ));
    }
    let backend = field("backend")?;
    let mode = Mode::read(&field("mode")?).ok_or_else(|| {
        tampered(
            "derivative.json",
            "its mode is not forward or reverse".to_owned(),
        )
    })?;
    let function = field("function")?;
    let inputs: Vec<String> = document
        .get("inputs")
        .and_then(Json::as_arr)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let compiler_ir_digest = field("compiler_ir_digest")?;
    let derivative_digest = field("derivative_digest")?;

    let lane = match backend.as_str() {
        "cgc" => Lane::Llvm,
        "native" => Lane::Expression,
        other => {
            return Err(tampered(
                "derivative.json",
                format!("`{}` is not a backend", safe(other)),
            ))
        }
    };

    // The compiler's IR first: for the LLVM lane that is the module as read;
    // for the expression lane it is the primal, which has to be regenerated
    // from the problem before it can be compared.
    let program = match lane {
        Lane::Llvm => {
            let found = digest::sha256(stored.input.as_bytes());
            if found != compiler_ir_digest {
                return Err(tampered(
                    "compiler_ir_digest",
                    format!(
                        "module.ll digests to {found}, the document records {compiler_ir_digest}"
                    ),
                ));
            }
            Program::LlvmIr(stored.input.clone())
        }
        Lane::Expression => Program::CatalystIr(crate::problem::parse(&stored.input)?),
    };
    let rules = match (&stored.rules, lane) {
        (Some(text), Lane::Llvm) => Some(Rules {
            text: text.clone(),
            bindings: rules::parse(text)?,
        }),
        _ => None,
    };
    let request = Request {
        function: function.clone(),
        inputs,
        at: at.to_vec(),
        mode: Some(mode),
        rules,
        source: None,
    };
    let artifact = backend_for(&program).differentiate(&program, &request)?;
    if lane == Lane::Expression {
        let found = digest::sha256(artifact.primal_cir.as_bytes());
        if found != compiler_ir_digest {
            return Err(tampered(
                "compiler_ir_digest",
                format!("the primal regenerated from problem.json digests to {found}, the document records {compiler_ir_digest}"),
            ));
        }
    }
    let found = digest::sha256(artifact.derivative_cir.as_bytes());
    if found != derivative_digest {
        return Err(tampered(
            "derivative_digest",
            format!("the regenerated derivative digests to {found}, the document records {derivative_digest}"),
        ));
    }
    // The listings beside the document are checked whole, not only through
    // the digests: `primal.cir` and `derivative.cir` against what was just
    // regenerated, `fixtures.json` against the validation digest the
    // document recorded over it. A reader trusts the files, so the files
    // that run must be the files that were shown. The report is prose about
    // the fixtures and is not checked.
    let validation_digest = field("validation_digest")?;
    for (name, stored_text) in &stored.listings {
        let expected = match name.as_str() {
            "primal.cir" => Some(&artifact.primal_cir),
            "derivative.cir" => Some(&artifact.derivative_cir),
            _ => None,
        };
        let same = match expected {
            Some(text) => text == stored_text,
            None => digest::sha256(stored_text.as_bytes()) == validation_digest,
        };
        if !same {
            return Err(tampered(
                name,
                "the file differs from the one this artifact wrote".to_owned(),
            ));
        }
    }
    check_point(&function, &artifact.parameters, at)?;
    let (value, gradient) = artifact
        .declared()
        .map_err(|fault| faulted(&function, &fault))?;
    Ok((artifact, value, gradient))
}

/// A name from a request, in a form a message can carry.
fn safe(name: &str) -> String {
    let mut out: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    if name.chars().count() > 32 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAT: &str = "target triple = \"x86_64-unknown-linux-gnu\"\n\
        define double @heat(double %x, double %y) {\n\
        start:\n\
        \x20 %c = fcmp ogt double %y, 1.000000e+00\n\
        \x20 %a = fmul double %y, 8.000000e-01\n\
        \x20 %s = select i1 %c, double %a, double %y\n\
        \x20 %e = call double @llvm.exp.f64(double %s)\n\
        \x20 %r = fmul double %x, %e\n\
        \x20 ret double %r\n\
        }\n\
        declare double @llvm.exp.f64(double)\n\
        !llvm.ident = !{!0}\n\
        !0 = !{!\"rustc version 1.98.0\"}\n";

    fn request(function: &str, inputs: &[&str], at: &[f64], mode: Option<Mode>) -> Request {
        Request {
            function: function.to_owned(),
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
            at: at.to_vec(),
            mode,
            rules: None,
            source: None,
        }
    }

    #[test]
    fn a_select_goes_reverse_and_verifies_against_the_closed_form() {
        let program = Program::LlvmIr(HEAT.to_owned());
        let validated = differentiate(&program, &request("heat", &["x", "y"], &[1.5, 2.0], None))
            .expect("validated");
        assert_eq!(validated.artifact.mode(), Mode::Reverse);
        assert_eq!(validated.points(), 5);
        let e = (1.6f64).exp();
        assert!((validated.value - 1.5 * e).abs() < 1e-12);
        assert!((validated.gradient[0] - e).abs() < 1e-12);
        assert!((validated.gradient[1] - 1.5 * e * 0.8).abs() < 1e-12);
        let files = validated
            .artifact
            .files()
            .expect("validated artifacts have files");
        assert_eq!(files.len(), 6);
        assert_eq!(files[1].1, HEAT);
        let document = Json::parse(&files[0].1).expect("json");
        assert_eq!(
            document.path("provenance.compiler").and_then(Json::as_str),
            Some("rustc version 1.98.0")
        );
        assert_eq!(
            document.get("compiler_ir_digest").and_then(Json::as_str),
            Some(digest::sha256(HEAT.as_bytes()).as_str())
        );
        assert!(files[5].1.contains(artifact::SENTENCE));
    }

    #[test]
    fn a_forced_mode_the_engine_refuses_is_the_engines_refusal() {
        let looped = "define double @spin(double %x, i64 %n) {\n\
            start:\n\
            \x20 br label %loop\n\
            loop:\n\
            \x20 %i = phi i64 [ 0, %start ], [ %j, %loop ]\n\
            \x20 %acc = phi double [ 0.000000e+00, %start ], [ %next, %loop ]\n\
            \x20 %next = fadd double %acc, %x\n\
            \x20 %j = add i64 %i, 1\n\
            \x20 %more = icmp slt i64 %j, %n\n\
            \x20 br i1 %more, label %loop, label %done\n\
            done:\n\
            \x20 ret double %next\n\
            }\n";
        let program = Program::LlvmIr(looped.to_owned());
        let forced = differentiate(
            &program,
            &request("spin", &["x"], &[0.5, 4.0], Some(Mode::Reverse)),
        );
        assert_eq!(
            forced.err().expect("refused").code,
            "catalyst.branching_control_flow"
        );
        let chosen =
            differentiate(&program, &request("spin", &["x"], &[0.5, 4.0], None)).expect("forward");
        assert_eq!(chosen.artifact.mode(), Mode::Forward);
        assert_eq!(chosen.value, 2.0);
        assert_eq!(chosen.gradient, vec![4.0]);
    }

    #[test]
    fn every_refusal_of_the_request_has_its_code() {
        let program = Program::LlvmIr(HEAT.to_owned());
        let code = |r: Request| differentiate(&program, &r).err().expect("refused").code;
        assert_eq!(
            code(request("drag", &["x"], &[1.0], None)),
            "catalyst.llvm_function_not_found"
        );
        assert_eq!(
            code(request("heat", &["q"], &[1.0, 2.0], None)),
            "catalyst.unknown_name"
        );
        assert_eq!(
            code(request("heat", &["x"], &[1.0], None)),
            "catalyst.wrong_arity"
        );
        assert_eq!(
            code(request("heat", &["x"], &[f64::INFINITY, 2.0], None)),
            "catalyst.llvm_argument_invalid"
        );
        assert_eq!(
            code(request("heat", &[], &[1.0, 2.0], None)),
            "catalyst.usage"
        );
        let prose = Program::LlvmIr("not a module".to_owned());
        assert_eq!(
            differentiate(&prose, &request("heat", &["x"], &[1.0], None))
                .err()
                .expect("refused")
                .code,
            "catalyst.llvm_syntax"
        );
    }

    #[test]
    fn a_wrong_rule_is_caught_by_the_verifier_not_believed() {
        let text = "define double @cube(double %x) {\n\
            start:\n\
            \x20 %a = fmul double %x, %x\n\
            \x20 %b = fmul double %a, %x\n\
            \x20 ret double %b\n\
            }\n\
            define double @wrong(double %x) {\n\
            start:\n\
            \x20 %b = fmul double %x, 2.000000e+00\n\
            \x20 ret double %b\n\
            }\n\
            define double @f(double %x) {\n\
            start:\n\
            \x20 %c = call double @cube(double %x)\n\
            \x20 ret double %c\n\
            }\n";
        let rules_text = "{\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":\"cube\",\"derivative\":\"wrong\"}]}";
        let mut r = request("f", &["x"], &[1.3], None);
        r.rules = Some(Rules {
            text: rules_text.to_owned(),
            bindings: rules::parse(rules_text).expect("reads"),
        });
        let refused = differentiate(&Program::LlvmIr(text.to_owned()), &r)
            .err()
            .expect("refused");
        assert_eq!(refused.code, "catalyst.derivative_disagrees");
        assert!(refused.detail.contains('x'));
    }

    #[test]
    fn describe_names_the_two_backends_in_order() {
        let d = describe();
        let names: Vec<&str> = d
            .get("backends")
            .and_then(Json::as_arr)
            .expect("backends")
            .iter()
            .filter_map(|b| b.get("name").and_then(Json::as_str))
            .collect();
        assert_eq!(names, ["native", "cgc"]);
    }
}
