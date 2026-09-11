//! `catalyst differentiate`: the command layer over the Catalyst Gradient
//! Compiler.
//!
//! `docs/interface.md` §12.2, §12.6 and §12.7. This file is where files are
//! read and written -- through [`crate::paths`], under the path rule, as for
//! every command -- and [`crate::gradient_compiler`] is where nothing is.
//! The order of operations here is the whole of the command's safety: every
//! input is read and every check is made before the output directory is
//! created, so a refused command leaves no directory behind, and an artifact
//! is written only after the verifier has passed it.

use crate::cli::{Args, Refused};
use crate::gradient_compiler::{self, engine::Mode, Program, Request, Rules, StoredArtifact};
use crate::json::{obj, s, Json};
use crate::paths;
use crate::problem::{Computation, Problem};

/// `catalyst differentiate …`. Returns the one JSON object the command prints.
pub fn run(args: &Args) -> Result<String, Refused> {
    if args.has("describe") {
        let mut members = vec![("ok".to_owned(), Json::Bool(true))];
        if let Json::Obj(pairs) = gradient_compiler::describe() {
            members.extend(pairs);
        }
        return Ok(Json::Obj(members).render());
    }
    if args.bare("llvm") {
        return llvm(args);
    }
    if args.bare("native") {
        return native(args);
    }
    if args.bare("run") {
        return rerun(args);
    }
    Err(Refused::usage(
        "`catalyst differentiate` needs a subcommand: `llvm --module FILE --function NAME \
         --inputs a,b --at v1,v2 --out DIR`, `native --problem FILE --out DIR`, `run \
         --artifact DIR --at v1,v2`, or `--describe`",
    ))
}

/// `--at v1,…,vn`.
fn point(text: &str) -> Result<Vec<f64>, Refused> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        let value: f64 = part.parse().map_err(|_| {
            Refused::new(
                "catalyst.llvm_argument_invalid",
                "--at holds a value that is not a number",
                "give --at one number per parameter, comma separated, for example 1.5,2",
            )
        })?;
        if !value.is_finite() {
            return Err(Refused::new(
                "catalyst.llvm_argument_invalid",
                "--at holds a value that is not finite",
                "give every parameter a finite value",
            ));
        }
        out.push(value);
    }
    Ok(out)
}

/// `--inputs a,b`.
fn names(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_start_matches('%').to_owned())
        .collect()
}

fn mode(args: &Args) -> Result<Option<Mode>, Refused> {
    match args.value("mode") {
        None => Ok(None),
        Some(word) => Mode::read(word)
            .map(Some)
            .ok_or_else(|| Refused::usage("--mode is `forward` or `reverse`")),
    }
}

/// `--rules FILE`, read and parsed.
fn rules(args: &Args) -> Result<Option<Rules>, Refused> {
    match args.value("rules") {
        None => Ok(None),
        Some(named) => {
            let text = paths::read_input(named)?;
            let bindings = gradient_compiler::rules::parse(&text)?;
            Ok(Some(Rules { text, bindings }))
        }
    }
}

/// `--source FILE`: digested, and its base name kept. Nothing else.
fn source(args: &Args) -> Result<Option<(String, String)>, Refused> {
    match args.value("source") {
        None => Ok(None),
        Some(named) => {
            let text = paths::read_input(named)?;
            let base = named.rsplit(['/', '\\']).next().unwrap_or(named).to_owned();
            Ok(Some((
                base,
                gradient_compiler::digest::sha256(text.as_bytes()),
            )))
        }
    }
}

/// The answer of a differentiation that wrote an artifact.
fn written(
    out_named: &str,
    validated: &gradient_compiler::Validated,
    files: &[(&'static str, String)],
) -> String {
    let mut members = vec![("ok", Json::Bool(true)), ("out", s(out_named))];
    members.extend(
        validated
            .artifact
            .answer_members(validated.value, &validated.gradient),
    );
    members.push(("validation", validated.validation_json()));
    members.push(("assurance", validated.assurance_json()));
    members.push((
        "files",
        Json::Arr(files.iter().map(|(name, _)| s(name)).collect()),
    ));
    obj(members).render()
}

/// Validate, then create the directory, then write: in that order.
fn write_artifact(
    out_named: &str,
    validated: &gradient_compiler::Validated,
) -> Result<String, Refused> {
    let files = validated
        .artifact
        .files()
        .expect("a validated artifact has files");
    let out = paths::prepare_out_dir(out_named)?;
    for (name, contents) in &files {
        paths::write_file(&out.full, name, contents)?;
    }
    Ok(written(out_named, validated, &files))
}

fn llvm(args: &Args) -> Result<String, Refused> {
    const COMMAND: &str = "differentiate llvm";
    let module_named = args.required("module", COMMAND)?;
    let function = args.required("function", COMMAND)?;
    let inputs = args.required("inputs", COMMAND)?;
    let at = args.required("at", COMMAND)?;
    let out_named = args.required("out", COMMAND)?;
    // The output path is checked before anything is read, so that a bad
    // destination is refused before the work rather than after it.
    paths::resolve(out_named)?;
    let mode = mode(args)?;
    let text = paths::read_input(module_named)?;
    let rules = rules(args)?;
    let source = source(args)?;
    let request = Request {
        function: function.to_owned(),
        inputs: names(inputs),
        at: point(at)?,
        mode,
        rules,
        source,
    };
    let validated = gradient_compiler::differentiate(&Program::LlvmIr(text), &request)?;
    write_artifact(out_named, &validated)
}

fn native(args: &Args) -> Result<String, Refused> {
    const COMMAND: &str = "differentiate native";
    let named = args.required("problem", COMMAND)?;
    let out_named = args.required("out", COMMAND)?;
    paths::resolve(out_named)?;
    let mode = mode(args)?;
    let problem = crate::problem::parse(&paths::read_input(named)?)?;
    if problem.is_llvm() {
        return Err(Refused::usage(
            "`catalyst differentiate native` takes a problem of the expression language; a \
             computation of kind llvm goes to `catalyst differentiate llvm`, or to `catalyst \
             eval`",
        ));
    }
    let request = Request {
        function: problem.name.clone(),
        inputs: problem.params.clone(),
        at: problem.at().to_vec(),
        mode,
        rules: None,
        source: None,
    };
    let mut validated =
        gradient_compiler::differentiate(&Program::CatalystIr(problem.clone()), &request)?;
    // The numbers are `catalyst eval`'s, from the same call it makes: one
    // computation, one spelling, whichever command printed it.
    let answer = crate::api::gradient(&problem.function, problem.at()).map_err(Refused::from)?;
    validated.value = answer.value;
    validated.gradient = answer.gradient;
    write_artifact(out_named, &validated)
}

fn rerun(args: &Args) -> Result<String, Refused> {
    const COMMAND: &str = "differentiate run";
    let artifact = args.required("artifact", COMMAND)?;
    let at = point(args.required("at", COMMAND)?)?;
    let inside = |name: &str| format!("{}/{name}", artifact.trim_end_matches('/'));
    let document = paths::read_input(&inside("derivative.json"))?;
    // Which input file the artifact keeps is the document's to say.
    let backend = Json::parse(&document)
        .ok()
        .and_then(|d| d.get("backend").and_then(Json::as_str).map(str::to_owned))
        .unwrap_or_default();
    let input_file = if backend == "native" {
        "problem.json"
    } else {
        "module.ll"
    };
    let input = paths::read_input(&inside(input_file))?;
    let rules = match paths::resolve(&inside("rules.json")) {
        Ok(resolved) if resolved.full.is_file() => Some(paths::read_input(&inside("rules.json"))?),
        _ => None,
    };
    // Every listing the artifact wrote must still be there and still be what
    // the artifact regenerates; a missing one is as tampered as an edited one.
    let mut listings = Vec::new();
    for name in ["primal.cir", "derivative.cir", "fixtures.json"] {
        listings.push((name.to_owned(), paths::read_input(&inside(name))?));
    }
    let stored = StoredArtifact {
        document,
        input,
        rules,
        listings,
    };
    let (artifact, value, gradient) = gradient_compiler::rerun(&stored, &at)?;
    let mut members = vec![
        ("ok", Json::Bool(true)),
        ("artifact_verified", Json::Bool(true)),
    ];
    members.extend(artifact.answer_members(value, &gradient));
    Ok(obj(members).render())
}

/// `catalyst eval` of a problem whose computation is an LLVM module: the
/// module is read under the path rule, the problem's inputs and fixed values
/// are matched to the function's parameters, and the Catalyst Gradient
/// Compiler differentiates and validates it.
pub fn evaluate_llvm_problem(problem: &Problem) -> Result<Vec<(&'static str, Json)>, Refused> {
    let Computation::Llvm {
        module,
        function,
        fixed,
    } = &problem.computation
    else {
        return Err(Refused::usage(
            "evaluate_llvm_problem takes a problem of kind llvm",
        ));
    };
    let text = paths::read_input(module)?;
    let parameters = gradient_compiler::parameters_of(&text, function)?;
    // Every parameter of the function has a value: a double from `inputs`,
    // an integer from `fixed`, and a parameter in neither is the document's
    // omission, named.
    let mut at = Vec::with_capacity(parameters.len());
    for (name, ty) in &parameters {
        let from_inputs = problem
            .params
            .iter()
            .position(|p| p == name)
            .map(|i| problem.inputs[i]);
        let from_fixed = fixed.iter().find(|(k, _)| k == name).map(|(_, v)| *v);
        let value = match (ty, from_inputs, from_fixed) {
            (crate::ir::Ty::Real, Some(x), _) => x,
            (_, _, Some(x)) => x,
            (crate::ir::Ty::Real, None, None) => {
                return Err(Refused::new(
                    "catalyst.problem_incomplete",
                    format!("the parameter `{name}` of `{function}` has no input value"),
                    "give every double parameter an entry in `inputs` and in `domains`",
                ))
            }
            (_, _, None) => {
                return Err(Refused::new(
                    "catalyst.problem_incomplete",
                    format!(
                        "the integer parameter `{name}` of `{function}` has no value in \
                         `computation.fixed`"
                    ),
                    "give every integer parameter a whole number in `computation.fixed`; it \
                     is held fixed",
                ))
            }
        };
        at.push(value);
    }
    let request = Request {
        function: function.clone(),
        inputs: problem.params.clone(),
        at,
        mode: None,
        rules: None,
        source: None,
    };
    let validated = gradient_compiler::differentiate(&Program::LlvmIr(text), &request)?;
    let finite = validated.value.is_finite() && validated.gradient.iter().all(|g| g.is_finite());
    let gradient = Json::Obj(
        problem
            .params
            .iter()
            .cloned()
            .zip(validated.gradient.iter().map(|g| Json::Num(*g)))
            .collect(),
    );
    Ok(vec![
        ("value", Json::Num(validated.value)),
        ("gradient", gradient),
        ("finite", Json::Bool(finite)),
        ("cost", validated.cost_json()),
        ("assurance", validated.assurance_json()),
        ("validation", validated.validation_json()),
    ])
}
