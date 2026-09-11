//! The tool surface a model calls: `catalyst tools discover` and
//! `catalyst tools call`.
//!
//! `docs/interface.md` §5. Three tools, one request shape, one response shape,
//! and a discovery document that describes all of it well enough that a caller
//! never has to guess.
//!
//! # Discovery describes the failures too
//!
//! A tool list that only names the happy path leaves the caller to discover
//! every refusal by hitting it. So the document carries an `errors` array in
//! which each entry says what the code means and what to do about it, and a
//! `continuation` block saying what survives an interrupted call and how to
//! pick it up. Both are part of the surface, not documentation about it.
//!
//! # A model may not redefine success
//!
//! [`authority_refusal`] runs before any other validation, on the tool name and
//! on the top-level argument keys. `set_acceptance`, `set_tolerance`,
//! `approve_result` and `approve`, and the arguments `acceptance`, `tolerance`
//! and `approve_own_result`, are refused as `catalyst.authority_refused`
//! whatever else is wrong with the request. The reason it is first is that a
//! caller must not be able to learn *which* of its attempts was better formed:
//! the answer to "may I approve my own result" is no, not "no, and here is how
//! to spell the request properly".

use crate::cli::{Args, Refused};
use crate::gradient_compiler;
use crate::json::{obj, s, Json};
use crate::label::LABEL;
use crate::paths;
use crate::problem::{self, Computation, Problem};
use std::io::Read;

/// The schema of the discovery document.
pub const DISCOVERY_SCHEMA: &str = "catalyst.tools.v1";
/// The schema of a request to [`call`].
pub const REQUEST_SCHEMA: &str = "catalyst.tool-request.v1";
/// The schema of the answer [`call`] prints.
pub const RESPONSE_SCHEMA: &str = "catalyst.tool-response.v1";

/// The tools, in the order discovery lists them.
///
/// `discover` is one of them, and not only a separate subcommand, because the
/// adapter boundary of §6 offers exactly these five capabilities and translates
/// each one into a request this surface serves. A capability that translated
/// into a request `tools call` then refused would make the boundary a promise
/// the surface does not keep.
pub const TOOLS: &[&str] = &[
    "discover",
    "validate_problem",
    "evaluate",
    "export_go",
    "differentiate_llvm",
];

/// Tool names that would let a caller move the goalposts, and the top-level
/// argument keys that would do the same thing more quietly.
const FORBIDDEN_TOOLS: &[&str] = &[
    "set_acceptance",
    "set_tolerance",
    "approve_result",
    "approve",
];
const FORBIDDEN_ARGUMENTS: &[&str] = &["acceptance", "tolerance", "approve_own_result"];

/// `catalyst tools …`. Returns the one JSON object the command prints.
pub fn run(args: &Args) -> Result<String, Refused> {
    if args.has("discover") {
        return Ok(answer(discovery()));
    }
    if args.has("call") {
        let named = args.required("request", "tools call")?;
        return call(&read_request(named)?);
    }
    Err(Refused::usage(
        "`catalyst tools` needs a subcommand: `discover`, or `call --request FILE`",
    ))
}

/// The request document: a file inside the current directory, or `-` for
/// standard input. Standard input is not a path, so the path rule has nothing
/// to say about it; it is bounded instead, because a pipe has no size a caller
/// declared in advance.
fn read_request(named: &str) -> Result<String, Refused> {
    if named != "-" {
        return paths::read_input(named);
    }
    let mut text = String::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| request_invalid("standard input is not valid UTF-8 text"))?;
    if text.len() > MAX_REQUEST_BYTES {
        return Err(request_invalid(format!(
            "the request on standard input is over the {MAX_REQUEST_BYTES} byte limit"
        )));
    }
    Ok(text)
}

/// The largest request the surface reads.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

fn request_invalid(detail: impl Into<String>) -> Refused {
    Refused::new(
        "catalyst.tool_request_invalid",
        detail,
        "send one JSON object with `schema`: \"catalyst.tool-request.v1\", a string `tool` \
         naming one of discover, validate_problem, evaluate, export_go or \
         differentiate_llvm, and an `arguments` object",
    )
}

/// One document as the command prints it: `"ok":true` first, then the document
/// itself. The document is built without that member so it can also be carried
/// *inside* an answer -- as the result of the `discover` tool, or as part of a
/// prompt -- where a bare `ok` would be a claim about the wrong thing.
fn answer(document: Json) -> String {
    let mut members = vec![("ok".to_owned(), Json::Bool(true))];
    if let Json::Obj(pairs) = document {
        members.extend(pairs);
    }
    Json::Obj(members).render()
}

/// The discovery document of §5, including the label of §7.
pub fn discovery() -> Json {
    obj(vec![
        ("schema", s(DISCOVERY_SCHEMA)),
        ("version", Json::Num(1.0)),
        ("label", s(LABEL)),
        ("tools", Json::Arr(TOOLS.iter().map(|t| tool(t)).collect())),
        ("errors", errors()),
        ("continuation", continuation()),
    ])
}

/// A JSON-Schema-shaped description of one argument object.
fn schema_of(required: &[&str], properties: Vec<(&str, Json)>) -> Json {
    obj(vec![
        ("type", s("object")),
        (
            "required",
            Json::Arr(required.iter().map(|name| s(name)).collect()),
        ),
        ("properties", obj(properties)),
        ("additionalProperties", Json::Bool(false)),
    ])
}

fn field(kind: &str, description: &str) -> Json {
    obj(vec![("type", s(kind)), ("description", s(description))])
}

fn tool(name: &str) -> Json {
    let problem_field = field(
        "object",
        "a catalyst.problem.v1 or catalyst.problem.v2 document: schema, name, goal, \
         function, inputs, domains, and in v2 an optional computation",
    );
    let (description, request, response) = match name {
        "discover" => (
            "Return this document: the tools, their request and response schemas, the \
             refusals they can produce, and the rules for continuing interrupted work.",
            schema_of(&[], Vec::new()),
            schema_of(
                &[
                    "schema",
                    "version",
                    "label",
                    "tools",
                    "errors",
                    "continuation",
                ],
                vec![
                    ("schema", field("string", "catalyst.tools.v1")),
                    ("version", field("number", "the version of this surface")),
                    (
                        "label",
                        field("string", "what this boundary is, and is not"),
                    ),
                    (
                        "tools",
                        field("array", "one entry per tool, with its schemas"),
                    ),
                    (
                        "errors",
                        field("array", "each refusal, its meaning and its remedy"),
                    ),
                    (
                        "continuation",
                        field("object", "the rules for resuming interrupted work"),
                    ),
                ],
            ),
        ),
        "validate_problem" => (
            "Check a catalyst.problem.v1 or v2 document and report the parameters it \
             declares, without evaluating it or writing anything.",
            schema_of(&["problem"], vec![("problem", problem_field)]),
            schema_of(
                &["valid", "name", "parameters"],
                vec![
                    (
                        "valid",
                        field(
                            "boolean",
                            "always true; a document that is not valid is refused instead",
                        ),
                    ),
                    ("name", field("string", "the name the document declares")),
                    (
                        "parameters",
                        field("array", "the parameter names, in declaration order"),
                    ),
                ],
            ),
        ),
        "evaluate" => (
            "Evaluate the problem at its declared inputs and return the value and the \
             gradient, one partial derivative per parameter.",
            schema_of(&["problem"], vec![("problem", problem_field)]),
            schema_of(
                &["value", "gradient", "finite", "cost"],
                vec![
                    (
                        "value",
                        field(
                            "number",
                            "the value at the declared inputs, null when not finite",
                        ),
                    ),
                    (
                        "gradient",
                        field("object", "one partial derivative per parameter name"),
                    ),
                    (
                        "finite",
                        field("boolean", "false when the value or a partial is not finite"),
                    ),
                    (
                        "cost",
                        field(
                            "object",
                            "instructions executed by the primal and the adjoint",
                        ),
                    ),
                ],
            ),
        ),
        "differentiate_llvm" => (
            "Differentiate a function of a textual LLVM IR module through the Catalyst \
             Gradient Compiler, check the derivative against central finite differences, \
             and return the value and the gradient at the point. Nothing is written.",
            schema_of(
                &["module", "function", "inputs", "at"],
                vec![
                    (
                        "module",
                        field("string", "the textual LLVM IR, as a compiler wrote it"),
                    ),
                    (
                        "function",
                        field("string", "the define to differentiate, without the @"),
                    ),
                    (
                        "inputs",
                        field(
                            "array",
                            "the double parameters to differentiate with respect to, by name",
                        ),
                    ),
                    (
                        "at",
                        field("array", "one number per parameter, in declaration order"),
                    ),
                    (
                        "rules",
                        field(
                            "object",
                            "optional: a catalyst.derivative-rules.v1 document of custom rules",
                        ),
                    ),
                ],
            ),
            schema_of(
                &[
                    "backend",
                    "mode",
                    "function",
                    "inputs",
                    "at",
                    "value",
                    "gradient",
                    "finite",
                    "validation",
                    "assurance",
                ],
                vec![
                    ("backend", field("string", "cgc")),
                    ("mode", field("string", "forward or reverse, as chosen")),
                    ("function", field("string", "the function differentiated")),
                    ("inputs", field("array", "the inputs, in the order named")),
                    ("at", field("object", "every parameter at its value")),
                    (
                        "value",
                        field("number", "the value at the point, null when not finite"),
                    ),
                    (
                        "gradient",
                        field("object", "one partial derivative per named input"),
                    ),
                    (
                        "finite",
                        field("boolean", "false when a number is not finite"),
                    ),
                    (
                        "validation",
                        field("object", "the verifier's points, all of which passed"),
                    ),
                    (
                        "assurance",
                        field(
                            "object",
                            "which lane produced the numbers, and what it promises",
                        ),
                    ),
                ],
            ),
        ),
        _ => (
            "Write the standard-library-only Go export of the problem into an empty \
             directory inside the current directory, with fixtures and a report.",
            schema_of(
                &["problem", "out"],
                vec![
                    ("problem", problem_field),
                    (
                        "out",
                        field("string", "a relative path inside the current directory"),
                    ),
                ],
            ),
            schema_of(
                &["out", "files", "cases"],
                vec![
                    ("out", field("string", "the directory that was written")),
                    (
                        "files",
                        field("array", "the names of the files written into it"),
                    ),
                    (
                        "cases",
                        field("number", "how many fixture cases were generated"),
                    ),
                ],
            ),
        ),
    };
    obj(vec![
        ("name", s(name)),
        ("description", s(description)),
        ("request_schema", request),
        ("response_schema", response),
    ])
}

fn error(code: &str, meaning: &str, remedy: &str) -> Json {
    obj(vec![
        ("code", s(code)),
        ("meaning", s(meaning)),
        ("remedy", s(remedy)),
    ])
}

fn errors() -> Json {
    Json::Arr(vec![
        error(
            "catalyst.tool_request_invalid",
            "the request is not one JSON object of schema catalyst.tool-request.v1 with a \
             string tool and an arguments object",
            "send the request shape the tool list declares, and nothing else",
        ),
        error(
            "catalyst.tool_unknown",
            "the request names a tool this surface does not have",
            "call one of discover, validate_problem, evaluate, export_go or differentiate_llvm",
        ),
        error(
            "catalyst.authority_refused",
            "the request tried to redefine success, weaken a tolerance or approve its own \
             result, which no caller of this surface may do",
            "ask the operator to change the acceptance criteria; a result is approved outside \
             this surface or not at all",
        ),
        error(
            "catalyst.syntax",
            "the problem document is not readable as JSON, or a member has the wrong type",
            "write a catalyst.problem.v1 object with finite numbers throughout",
        ),
        error(
            "catalyst.problem_incomplete",
            "a parameter of the function has no input value or no domain",
            "give every parameter an entry in inputs and in domains",
        ),
        error(
            "catalyst.unknown_name",
            "an input or a domain names something the function does not declare",
            "remove the entry, or declare that name as a parameter of the function",
        ),
        error(
            "catalyst.input_out_of_domain",
            "an input lies outside the domain the document declares for it",
            "move the input inside [min, max], or widen the domain to include it",
        ),
        error(
            "catalyst.path_refused",
            "a path in the request leaves the current directory, or passes through a symbolic \
             link; nothing was written",
            "name a relative path inside the current directory, with no `..` component",
        ),
        error(
            "catalyst.path_not_empty",
            "the output directory of an export already has something in it",
            "name a directory that does not exist yet, or empty this one first",
        ),
        error(
            "catalyst.derivative_disagrees",
            "the Catalyst Gradient Compiler's derivative and Catalyst's finite-difference \
             verifier disagree at some point, so the derivative is not trusted",
            "the detail names the input, the point and both numbers; if a custom rule \
             applied, check it, otherwise report the module",
        ),
        error(
            "catalyst.llvm_unsupported",
            "the module uses something outside the subset this phase differentiates, \
             named with its line",
            "keep to scalar arithmetic, comparisons, select, branches, phis, loops and \
             the math intrinsics; memory and aggregates are not read in this phase",
        ),
        error(
            "catalyst.opaque_call",
            "the function calls something the engine cannot see through, and the module \
             was not executed",
            "define the callee in the module, or register a custom rule for it",
        ),
        error(
            "catalyst.artifact_tampered",
            "a derivative artifact no longer matches the digests recorded in it, so its \
             numbers cannot be trusted",
            "regenerate the artifact with `catalyst differentiate` rather than editing it",
        ),
        error(
            "catalyst.ai_unavailable",
            "the command `catalyst ai propose` runs the model with cannot be started on \
             this machine",
            "install that command, or name another with --command PATH; every manual \
             command works without it",
        ),
        error(
            "catalyst.computation_kind_unsupported",
            "a v2 problem names a computation kind other than catalyst-expression or llvm",
            "use kind catalyst-expression with a function, or kind llvm with a module and \
             a function name",
        ),
        error(
            "catalyst.portable_export_unavailable",
            "the problem is a computation of kind llvm, and there is no Go or R export of \
             an LLVM derivative in this phase",
            "export a catalyst-expression problem, or run the llvm one with `catalyst \
             differentiate llvm` and read its artifact",
        ),
        error(
            "catalyst.usage",
            "the command line does not fit the shape the command accepts: a missing form, \
             an unknown flag, or a flag without its value",
            "run `catalyst --help`, or `catalyst <command> --help`, for the accepted \
             arguments",
        ),
        error(
            "catalyst.not_a_terminal",
            "the interactive front end needs a terminal on standard input, and this run \
             has none",
            "run `catalyst tui` from a terminal, or drive it with --keys FILE, adding \
             --headless --transcript FILE for the plain-text renderer",
        ),
        error(
            "catalyst.tui_size_refused",
            "the terminal, or the --width and --height given, cannot hold the panelled \
             layout",
            "give the front end at least 70 columns by 16 rows, or fix the size with \
             --width and --height",
        ),
        error(
            "catalyst.adapter.request_invalid",
            "the document given to `catalyst adapter translate` is not one \
             catalyst.adapter-request.v1 object with an id and a capability",
            "send the request shape `catalyst adapter conformance` exercises, and \
             nothing else",
        ),
        error(
            "catalyst.adapter.capability_unsupported",
            "an adapter request names a capability this build does not offer",
            "call one of the capabilities the adapter's discovery document lists",
        ),
        error(
            "catalyst.adapter.arguments_invalid",
            "an adapter request's arguments do not fit the capability's declared schema",
            "send the argument shape the adapter's discovery document declares for that \
             capability",
        ),
        error(
            "catalyst.adapter.response_invalid",
            "a response is not shaped as catalyst.adapter-response.v1, which the \
             conformance run reports rather than hides",
            "report the request that produced it; `catalyst adapter conformance` \
             reproduces every case it checks",
        ),
        error(
            "catalyst.ai_pending",
            "the state file holds a call that did not finish, and the command was run \
             again without --resume",
            "run the same command again with --resume to continue that goal, or name \
             another state file",
        ),
        error(
            "catalyst.ai_state_mismatch",
            "the state file holds a pending call for a different goal than the one given",
            "resume with the goal saved in that file, or name a fresh state file for the \
             new goal",
        ),
        error(
            "catalyst.ai_arguments_invalid",
            "the provider argument template names no arguments at all, so there is \
             nothing to run the command with",
            "give --provider-args, or CATALYST_AI_ARGS, at least one argument; `catalyst \
             ai status` shows what is configured",
        ),
        error(
            "catalyst.ai_call_failed",
            "the model command started but did not exit successfully",
            "run the same command again with --resume to try the saved goal once more",
        ),
        error(
            "catalyst.proposal_refused",
            "the model's answer is not one valid problem document, so nothing was written",
            "ask again with a clearer goal, or write the problem by hand",
        ),
        error(
            "catalyst.rehearsal.configuration_changed",
            "rehearsal.json no longer matches the digest recorded in it, so a result \
             measured under it could not be compared with an earlier one",
            "restore the recorded configuration, or start a new rehearsal directory",
        ),
        error(
            "catalyst.rehearsal.nothing_to_reduce",
            "the replay result records no failure, so there is nothing to reduce",
            "replay under a disturbance that produces a failure first, then reduce the \
             result that run writes",
        ),
        error(
            "catalyst.rehearsal.target_not_local",
            "a rehearsal target names something other than a local executable, and \
             nothing outside this machine is ever driven",
            "name a local executable as the target",
        ),
        error(
            "catalyst.rehearsal.trace_invalid",
            "a recorded trace is not shaped as catalyst.trace.v1, or is internally \
             inconsistent",
            "import the trace again with `catalyst rehearse import`, or synthesise one \
             with `catalyst rehearse synth-trace`, and do not edit it",
        ),
        error(
            "catalyst.function_too_long",
            "the expression is over the byte limit a problem document may carry",
            "shorten the expression, or split the computation into smaller problems",
        ),
        error(
            "catalyst.wrong_arity",
            "a known function is called with the wrong number of arguments, or --at \
             gives the wrong number of values",
            "check the argument count against the function's declaration",
        ),
        error(
            "catalyst.cyclic_control_flow",
            "the function loops, and reverse mode cannot replay a loop without a \
             recorded trace",
            "the compiler falls back to forward mode by itself; ask for forward mode \
             explicitly with --mode forward to silence the note",
        ),
        error(
            "catalyst.branching_control_flow",
            "the function branches, and reverse mode in this build replays only \
             straight-line code",
            "the compiler falls back to forward mode by itself, or express the branch as \
             a select so it becomes straight-line",
        ),
        error(
            "catalyst.no_derivative_rule",
            "an operation in the program has no derivative rule in this build",
            "rewrite the computation in terms of operations that have rules, or register \
             a custom rule for this one",
        ),
        error(
            "catalyst.nothing_active",
            "the request marked no input as active, so every derivative asked for would \
             be zero by construction",
            "name at least one double parameter in --inputs",
        ),
        error(
            "catalyst.annotation_mismatch",
            "a pointer parameter was marked active, or a scalar was marked duplicated, \
             so the annotation and the type disagree",
            "use active for scalars and duplicated for pointers",
        ),
        error(
            "catalyst.unresolved_object",
            "a load or store reaches an object whose type could not be resolved, and \
             shadowing it would mean guessing whether it carries a derivative",
            "give the pointer parameter a type hint on the job",
        ),
        error(
            "catalyst.llvm_syntax",
            "the module is not readable as the textual LLVM IR this reader accepts, named \
             with its line",
            "hand Catalyst the textual IR a compiler wrote, unedited: `rustc \
             --emit=llvm-ir` or `clang -S -emit-llvm`; bitcode is not read",
        ),
        error(
            "catalyst.llvm_module_too_long",
            "the module is over the byte limit this phase reads",
            "hand Catalyst a module holding only the function and what it calls",
        ),
        error(
            "catalyst.llvm_function_not_found",
            "the module does not define the function named; the detail lists what it \
             does define",
            "name one of the functions the module defines",
        ),
        error(
            "catalyst.llvm_input_not_real",
            "an input named for differentiation is an integer parameter, and a \
             derivative is taken only with respect to a double",
            "name only double parameters in --inputs; integers are held fixed",
        ),
        error(
            "catalyst.llvm_argument_invalid",
            "a value in --at is not a number, is not finite, or is not a whole number an \
             integer parameter can hold",
            "give --at one finite number per parameter, comma separated, whole for \
             integer parameters",
        ),
        error(
            "catalyst.llvm_run_faulted",
            "running the function under the engine's interpreter faulted, or exceeded \
             its instruction budget",
            "check that the function terminates for these inputs and reads no memory",
        ),
        error(
            "catalyst.rule_invalid",
            "a derivative-rules document is not shaped as catalyst.derivative-rules.v1, \
             or names functions the module does not define with one double parameter",
            "write {\"schema\":\"catalyst.derivative-rules.v1\",\"rules\":[{\"function\":\
             NAME,\"derivative\":NAME}]} naming two functions defined in the module",
        ),
    ])
}

fn continuation() -> Json {
    obj(vec![
        (
            "rules",
            Json::Arr(vec![
                s(
                    "Every call is independent: nothing is remembered between two calls, and a \
                   refused call changes no state and writes no file.",
                ),
                s(
                    "An interrupted `catalyst ai propose` keeps its goal and its attempts in the \
                   file named by --state, with pending set to true.",
                ),
                s(
                    "While a state file is pending, running the same command without --resume is \
                   refused as catalyst.ai_pending rather than starting the work again.",
                ),
                s(
                    "Resuming continues the saved goal: the work already recorded is never \
                   silently discarded, and the attempts array grows rather than restarting.",
                ),
                s(
                    "Only approved data ever leaves the process: the goal, this document, and \
                   the validated fields of a context problem.",
                ),
            ]),
        ),
        ("resume", s("catalyst ai propose --state FILE --resume")),
    ])
}

/// Serve one request. The text is the whole document; nothing else is read.
pub fn call(text: &str) -> Result<String, Refused> {
    let document = Json::parse(text).map_err(|error| {
        request_invalid(format!("at byte {}, expected {}", error.at, error.what))
    })?;
    if !document.is_obj() {
        return Err(request_invalid("the request is not a JSON object"));
    }
    // First, and whatever else the document gets wrong.
    if let Some(refused) = authority_refusal(&document) {
        return Err(refused);
    }
    match document.get("schema").and_then(Json::as_str) {
        Some(REQUEST_SCHEMA) => {}
        Some(_) => {
            return Err(request_invalid(
                "the `schema` member is not \"catalyst.tool-request.v1\"",
            ))
        }
        None => return Err(request_invalid("the request has no string `schema` member")),
    }
    let Some(tool) = document.get("tool").and_then(Json::as_str) else {
        return Err(request_invalid("the request has no string `tool` member"));
    };
    let arguments = match document.get("arguments") {
        Some(value) if value.is_obj() => value,
        Some(_) => return Err(request_invalid("the `arguments` member is not an object")),
        None => return Err(request_invalid("the request has no `arguments` member")),
    };

    let result = match tool {
        "discover" => discovery(),
        "validate_problem" => validate(arguments)?,
        "evaluate" => evaluate(arguments)?,
        "export_go" => export_go(arguments)?,
        "differentiate_llvm" => differentiate_llvm(arguments)?,
        other => {
            return Err(Refused::new(
                "catalyst.tool_unknown",
                format!("`{}` is not a tool of this surface", safe(other)),
                "call one of discover, validate_problem, evaluate, export_go or \
                 differentiate_llvm; `catalyst tools discover` lists them with their schemas",
            ))
        }
    };
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("schema", s(RESPONSE_SCHEMA)),
        ("tool", s(tool)),
        ("result", result),
    ])
    .render())
}

/// The refusal of §5, if this request is one of the requests no caller may
/// make. Checked on the tool name and on the top-level argument keys, before
/// the schema, before the tool exists, before the arguments are a shape.
pub fn authority_refusal(document: &Json) -> Option<Refused> {
    let refusal = |what: &str, detail: String| {
        Some(Refused::new(
            "catalyst.authority_refused",
            detail,
            format!(
                "remove `{what}` from the request. success is defined by the operator's \
                 acceptance criteria, and a result is approved outside this surface or not \
                 at all; call validate_problem, evaluate, export_go or differentiate_llvm \
                 instead"
            ),
        ))
    };
    if let Some(tool) = document.get("tool").and_then(Json::as_str) {
        if let Some(name) = FORBIDDEN_TOOLS.iter().find(|f| **f == tool) {
            return refusal(
                name,
                format!(
                    "`{name}` would let the caller redefine success, weaken a check or \
                         approve its own result"
                ),
            );
        }
    }
    if let Some(pairs) = document.get("arguments").and_then(Json::as_obj) {
        for (key, _) in pairs {
            if let Some(name) = FORBIDDEN_ARGUMENTS.iter().find(|f| **f == key.as_str()) {
                return refusal(
                    name,
                    format!(
                        "the argument `{name}` would let the caller redefine success, \
                             weaken a check or approve its own result"
                    ),
                );
            }
        }
    }
    None
}

/// The `problem` argument, validated.
fn problem_argument(arguments: &Json) -> Result<Problem, Refused> {
    let Some(value) = arguments.get("problem") else {
        return Err(request_invalid(
            "the arguments have no `problem` member; every tool of this surface takes one",
        ));
    };
    problem::parse(&value.render())
}

fn validate(arguments: &Json) -> Result<Json, Refused> {
    let problem = problem_argument(arguments)?;
    Ok(obj(vec![
        ("valid", Json::Bool(true)),
        ("name", s(&problem.name)),
        (
            "parameters",
            Json::Arr(problem.params.iter().map(|p| s(p)).collect()),
        ),
    ]))
}

fn evaluate(arguments: &Json) -> Result<Json, Refused> {
    let problem = problem_argument(arguments)?;
    Ok(obj(evaluation(&problem)?))
}

/// Value, gradient, finiteness and cost, as `catalyst eval` prints them and as
/// the `evaluate` tool returns them. One computation, one spelling.
///
/// A v1 document gets exactly the four members it always did. A v2 document
/// adds `assurance`, which says which lane produced the numbers, and a
/// computation of kind `llvm` goes through the Catalyst Gradient Compiler and
/// adds `validation` as well.
pub fn evaluation(problem: &Problem) -> Result<Vec<(&'static str, Json)>, Refused> {
    match &problem.computation {
        Computation::Unstated => expression_evaluation(problem),
        Computation::Expression => {
            let mut members = expression_evaluation(problem)?;
            // The lane is stated, so its claim is checked: the same verifier
            // that stands behind every artifact stands behind this answer.
            let validated = gradient_compiler::differentiate(
                &gradient_compiler::Program::CatalystIr(problem.clone()),
                &gradient_compiler::Request {
                    function: problem.name.clone(),
                    inputs: problem.params.clone(),
                    at: problem.at().to_vec(),
                    mode: None,
                    rules: None,
                    source: None,
                },
            )?;
            members.push(("assurance", validated.assurance_json()));
            Ok(members)
        }
        Computation::Llvm { .. } => crate::differentiate::evaluate_llvm_problem(problem),
    }
}

/// The expression lane's numbers: [`crate::api::gradient`], and nothing else
/// in the path.
fn expression_evaluation(problem: &Problem) -> Result<Vec<(&'static str, Json)>, Refused> {
    let answer = crate::api::gradient(&problem.function, problem.at()).map_err(Refused::from)?;
    let finite = answer.value.is_finite() && answer.gradient.iter().all(|g| g.is_finite());
    let gradient = Json::Obj(
        problem
            .params
            .iter()
            .cloned()
            .zip(answer.gradient.iter().map(|g| Json::Num(*g)))
            .collect(),
    );
    Ok(vec![
        ("value", Json::Num(answer.value)),
        ("gradient", gradient),
        ("finite", Json::Bool(finite)),
        (
            "cost",
            obj(vec![
                ("primal_insts", Json::Num(answer.primal_insts as f64)),
                ("adjoint_insts", Json::Num(answer.adjoint_insts as f64)),
            ]),
        ),
    ])
}

/// The export tool.
///
/// The order here is the whole of its safety: the path is *checked* first so a
/// request that leaves the current directory is refused before anything is
/// read, then the problem is validated, and only then is the directory
/// created. A broken problem with `"out":"never-created"` must leave no
/// directory called `never-created` behind, which is exactly what creating it
/// any earlier would do.
fn export_go(arguments: &Json) -> Result<Json, Refused> {
    let Some(named) = arguments.get("out").and_then(Json::as_str) else {
        return Err(request_invalid(
            "the arguments of `export_go` have no string `out` member",
        ));
    };
    paths::resolve(named)?;
    let problem = problem_argument(arguments)?;
    if let Some(refused) = problem.portable_export_unavailable() {
        return Err(refused);
    }
    let out = paths::prepare_out_dir(named)?;
    let cases = crate::export::write_go_export(&problem, &out)?;
    Ok(obj(vec![
        ("out", s(named)),
        (
            "files",
            Json::Arr(crate::export::FILES.iter().map(|f| s(f)).collect()),
        ),
        ("cases", Json::Num(cases as f64)),
    ]))
}

/// The `differentiate_llvm` tool: the Catalyst Gradient Compiler over a
/// module carried *in* the request, so that nothing is read and nothing is
/// written. The verifier's tolerance is not among the arguments, and a
/// request that tries to carry one was refused before this function was
/// reached.
fn differentiate_llvm(arguments: &Json) -> Result<Json, Refused> {
    let Some(module) = arguments.get("module").and_then(Json::as_str) else {
        return Err(request_invalid(
            "the arguments of `differentiate_llvm` have no string `module` member holding \
             the LLVM IR text",
        ));
    };
    let Some(function) = arguments.get("function").and_then(Json::as_str) else {
        return Err(request_invalid(
            "the arguments of `differentiate_llvm` have no string `function` member",
        ));
    };
    let Some(inputs) = arguments.get("inputs").and_then(Json::as_arr) else {
        return Err(request_invalid(
            "the arguments of `differentiate_llvm` have no `inputs` array",
        ));
    };
    let mut names = Vec::new();
    for input in inputs {
        match input.as_str() {
            Some(name) => names.push(name.to_owned()),
            None => {
                return Err(request_invalid(
                    "every entry of `inputs` is a string naming a parameter",
                ))
            }
        }
    }
    let Some(at) = arguments.get("at").and_then(Json::as_arr) else {
        return Err(request_invalid(
            "the arguments of `differentiate_llvm` have no `at` array",
        ));
    };
    let mut point = Vec::new();
    for value in at {
        match value.as_f64() {
            Some(x) => point.push(x),
            None => return Err(request_invalid("every entry of `at` is a finite number")),
        }
    }
    let rules = match arguments.get("rules") {
        None => None,
        Some(value) if value.is_obj() => {
            let text = value.render();
            Some(gradient_compiler::Rules {
                bindings: gradient_compiler::rules::parse(&text)?,
                text,
            })
        }
        Some(_) => {
            return Err(request_invalid(
                "the `rules` member is a catalyst.derivative-rules.v1 object",
            ))
        }
    };
    let request = gradient_compiler::Request {
        function: function.to_owned(),
        inputs: names,
        at: point,
        mode: None,
        rules,
        source: None,
    };
    let validated = gradient_compiler::differentiate(
        &gradient_compiler::Program::LlvmIr(module.to_owned()),
        &request,
    )?;
    let mut members = validated
        .artifact
        .answer_members(validated.value, &validated.gradient);
    members.push(("validation", validated.validation_json()));
    members.push(("assurance", validated.assurance_json()));
    Ok(obj(members))
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

    fn simple() -> String {
        "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
         \"function\":\"func p(x) = x * x\",\"inputs\":{\"x\":3},\
         \"domains\":{\"x\":{\"min\":0,\"max\":9,\"unit\":\"\"}}}"
            .to_string()
    }

    fn request(tool: &str, arguments: &str) -> String {
        format!("{{\"schema\":\"{REQUEST_SCHEMA}\",\"tool\":\"{tool}\",\"arguments\":{arguments}}}")
    }

    #[test]
    fn discovery_carries_the_label_the_adapter_boundary_declares() {
        let document = discovery();
        assert_eq!(document.get("label").and_then(Json::as_str), Some(LABEL));
    }

    #[test]
    fn evaluate_answers_with_the_value_and_the_gradient() {
        let answer = call(&request(
            "evaluate",
            &format!("{{\"problem\":{}}}", simple()),
        ))
        .expect("served");
        let json = Json::parse(&answer).expect("one object");
        assert_eq!(
            json.get("schema").and_then(Json::as_str),
            Some(RESPONSE_SCHEMA)
        );
        assert_eq!(
            json.get("result")
                .and_then(|r| r.get("value"))
                .and_then(Json::as_f64),
            Some(9.0)
        );
    }

    #[test]
    fn authority_is_refused_before_the_request_is_otherwise_read() {
        // No schema, no arguments object, an unknown tool: still this refusal.
        let broken = "{\"tool\":\"approve\",\"arguments\":7}";
        assert_eq!(
            call(broken).expect_err("refused").code,
            "catalyst.authority_refused"
        );
        let sneaked = request("evaluate", "{\"tolerance\":{\"relative\":1.0}}");
        assert_eq!(
            call(&sneaked).expect_err("refused").code,
            "catalyst.authority_refused"
        );
    }

    #[test]
    fn an_unknown_tool_is_told_apart_from_a_malformed_request() {
        assert_eq!(
            call(&request("nonexistent", "{}"))
                .expect_err("refused")
                .code,
            "catalyst.tool_unknown"
        );
        assert_eq!(
            call("not json at all").expect_err("refused").code,
            "catalyst.tool_request_invalid"
        );
        assert_eq!(
            call(&request("evaluate", "5")).expect_err("refused").code,
            "catalyst.tool_request_invalid"
        );
    }

    #[test]
    fn the_refusal_of_the_problem_reader_reaches_the_caller_unchanged() {
        let incomplete = "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
                          \"function\":\"func p(x, y) = x * y\",\"inputs\":{\"x\":1},\
                          \"domains\":{\"x\":{\"min\":0,\"max\":2,\"unit\":\"\"}}}";
        assert_eq!(
            call(&request(
                "evaluate",
                &format!("{{\"problem\":{incomplete}}}")
            ))
            .expect_err("refused")
            .code,
            "catalyst.problem_incomplete"
        );
    }
}
