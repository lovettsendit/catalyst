//! The versioned protocol of the boundary: request, response, capability
//! discovery, faults and continuation, as plain data and total functions over
//! it.
//!
//! `docs/interface.md` §6. Everything here is a transformation of one JSON
//! value into another. There is no I/O of any kind in this file -- nothing is
//! read, nothing is written, nothing is started, no address is opened -- which
//! is what makes every function below testable by comparing two values, and
//! what makes the conformance fixtures of [`super::conformance`] deterministic
//! without any model at all.
//!
//! # The boundary offers exactly the capabilities the surface serves
//!
//! [`CAPABILITIES`] *is* [`crate::tools::TOOLS`]. It is not a copy that has to
//! be kept in step with it: a capability that translated into a request the
//! tool surface then refused would make this boundary a promise somebody else
//! has to keep, and the way to make that impossible is to have one list.

use crate::cli::Refused;
use crate::json::{obj, s, Json};
use crate::label::LABEL;
use crate::problem;
use crate::tools;

/// The schema of a request arriving at the boundary.
pub const REQUEST_SCHEMA: &str = "catalyst.adapter-request.v1";
/// The schema of an answer leaving it.
pub const RESPONSE_SCHEMA: &str = "catalyst.adapter-response.v1";
/// The schema of the boundary's own capability discovery.
pub const DISCOVERY_SCHEMA: &str = "catalyst.adapter-discovery.v1";
/// The schema of the conformance report.
pub const CONFORMANCE_SCHEMA: &str = "catalyst.adapter-conformance.v1";

/// The capabilities of the boundary: exactly the tools of the surface behind
/// it, in the same order.
pub const CAPABILITIES: &[&str] = tools::TOOLS;

/// The remedy of an unsupported capability. It names every entry of
/// [`CAPABILITIES`], and a unit test below holds it to that: a remedy that has
/// fallen behind the list it describes sends the reader somewhere that is not
/// there any more.
const SUPPORTED: &str = "name one of the supported capabilities -- discover, validate_problem, \
                         evaluate, export_go or differentiate_llvm -- in the `capability` \
                         member";

/// What the boundary itself can refuse.
///
/// A refusal that comes from further in -- an unreadable problem document, a
/// path that leaves the current directory -- keeps its own code and travels
/// through unchanged. These five are the boundary's own, and the discovery
/// document publishes all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    CapabilityUnsupported,
    RequestInvalid,
    ResponseInvalid,
    ArgumentsInvalid,
    Authority,
}

/// Every fault, so the discovery document and the error-mapping fixture are
/// built from the same list rather than from two lists that agree today.
pub const FAULTS: &[Fault] = &[
    Fault::CapabilityUnsupported,
    Fault::RequestInvalid,
    Fault::ResponseInvalid,
    Fault::ArgumentsInvalid,
    Fault::Authority,
];

impl Fault {
    /// The name the protocol uses for this fault.
    pub fn name(self) -> &'static str {
        match self {
            Fault::CapabilityUnsupported => "capability_unsupported",
            Fault::RequestInvalid => "request_invalid",
            Fault::ResponseInvalid => "response_invalid",
            Fault::ArgumentsInvalid => "arguments_invalid",
            Fault::Authority => "authority",
        }
    }

    /// The `catalyst.…` code it maps to. This mapping is the whole of the
    /// error contract: a caller matches on these and never on prose.
    pub fn code(self) -> &'static str {
        match self {
            Fault::CapabilityUnsupported => "catalyst.adapter.capability_unsupported",
            Fault::RequestInvalid => "catalyst.adapter.request_invalid",
            Fault::ResponseInvalid => "catalyst.adapter.response_invalid",
            Fault::ArgumentsInvalid => "catalyst.adapter.arguments_invalid",
            Fault::Authority => "catalyst.authority_refused",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Fault::CapabilityUnsupported => {
                "the request names a capability this boundary does not offer, or names none"
            }
            Fault::RequestInvalid => {
                "the request is not one JSON object of schema catalyst.adapter-request.v1 \
                 with a string capability"
            }
            Fault::ResponseInvalid => {
                "the answer is not one JSON object of schema catalyst.adapter-response.v1, \
                 or it is neither a result nor a fully described error"
            }
            Fault::ArgumentsInvalid => "the arguments do not carry what the named capability needs",
            Fault::Authority => {
                "the request tried to redefine success, weaken a tolerance or approve its own \
                 result, which no caller of this boundary may do"
            }
        }
    }

    pub fn remedy(self) -> &'static str {
        match self {
            Fault::CapabilityUnsupported => SUPPORTED,
            Fault::RequestInvalid => {
                "send one JSON object with `schema`: \"catalyst.adapter-request.v1\", a string \
                 `capability` and an `arguments` object"
            }
            Fault::ResponseInvalid => {
                "answer with `schema`: \"catalyst.adapter-response.v1\", the capability, a \
                 boolean `ok`, and either a `result` object or an `error` object carrying \
                 code, detail and remedy"
            }
            Fault::ArgumentsInvalid => {
                "`catalyst adapter conformance` and the discovery document give the arguments \
                 of every capability; discover takes none, validate_problem and evaluate take \
                 a problem, export_go a problem and a relative out path, and \
                 differentiate_llvm a module, a function, an inputs array and an at array"
            }
            Fault::Authority => {
                "ask the operator to change the acceptance criteria; a result is approved \
                 outside this boundary or not at all"
            }
        }
    }

    /// This fault as the refusal a caller receives.
    pub fn refuse(self, detail: impl Into<String>) -> Refused {
        Refused::new(self.code(), detail, self.remedy())
    }

    /// This fault as it appears in the discovery document.
    pub fn describe(self) -> Json {
        obj(vec![
            ("name", s(self.name())),
            ("code", s(self.code())),
            ("meaning", s(self.meaning())),
            ("remedy", s(self.remedy())),
        ])
    }
}

/// A request that has been read but not yet translated.
#[derive(Clone, Debug)]
pub struct Request {
    pub capability: String,
    pub arguments: Json,
}

/// An answer that has been read back and checked.
#[derive(Clone, Debug)]
pub struct Response {
    pub capability: String,
    pub ok: bool,
}

/// Read one `catalyst.adapter-request.v1` document.
///
/// An unsupported capability is *not* refused here: reading and supporting are
/// two questions, and answering them separately is what lets the refusal say
/// which one failed.
pub fn read_request(text: &str) -> Result<Request, Refused> {
    let document = read_object(text, Fault::RequestInvalid)?;
    expect_schema(&document, REQUEST_SCHEMA, Fault::RequestInvalid)?;
    let Some(capability) = document.get("capability").and_then(Json::as_str) else {
        return Err(Fault::RequestInvalid.refuse("the request has no string `capability` member"));
    };
    let arguments = match document.get("arguments") {
        None => Json::Obj(Vec::new()),
        Some(value) if value.is_obj() => value.clone(),
        Some(_) => {
            return Err(Fault::RequestInvalid.refuse("the `arguments` member is not an object"))
        }
    };
    Ok(Request {
        capability: capability.to_owned(),
        arguments,
    })
}

/// Turn a read request into the `catalyst.tool-request.v1` object the tool
/// surface serves.
///
/// The problem is validated on the way through and rebuilt from the fields
/// that validation accepted, so what leaves this function is a request the
/// surface cannot refuse for a reason the caller has not already been told.
pub fn translate(request: &Request) -> Result<Json, Refused> {
    // Authority first, and before the capability is even looked up: the answer
    // to "may I approve my own result" is no, not "no, and here is how to
    // spell it properly".
    let candidate = tool_request(&request.capability, request.arguments.clone());
    if let Some(refused) = tools::authority_refusal(&candidate) {
        return Err(refused);
    }
    if !CAPABILITIES.contains(&request.capability.as_str()) {
        return Err(Fault::CapabilityUnsupported.refuse(format!(
            "`{}` is not a capability of this boundary",
            safe(&request.capability)
        )));
    }
    let arguments = match request.capability.as_str() {
        "discover" => Json::Obj(Vec::new()),
        "export_go" => {
            let problem = validated_problem(&request.arguments)?;
            let Some(out) = request.arguments.get("out").and_then(Json::as_str) else {
                return Err(Fault::ArgumentsInvalid
                    .refuse("`export_go` needs a string `out` member naming where to write"));
            };
            obj(vec![("problem", problem), ("out", s(out))])
        }
        "differentiate_llvm" => differentiate_arguments(&request.arguments)?,
        _ => obj(vec![("problem", validated_problem(&request.arguments)?)]),
    };
    Ok(tool_request(&request.capability, arguments))
}

/// The arguments of `differentiate_llvm`, checked for shape and rebuilt from
/// the four members the tool reads and the optional rules: a string module,
/// a string function, an array of input names, an array of numbers. The
/// module is text carried in the request, and it is not read here -- the
/// Catalyst Gradient Compiler reads it when the surface serves the call.
fn differentiate_arguments(arguments: &Json) -> Result<Json, Refused> {
    let Some(module) = arguments.get("module").and_then(Json::as_str) else {
        return Err(Fault::ArgumentsInvalid
            .refuse("`differentiate_llvm` needs a string `module` member holding LLVM IR text"));
    };
    let Some(function) = arguments.get("function").and_then(Json::as_str) else {
        return Err(
            Fault::ArgumentsInvalid.refuse("`differentiate_llvm` needs a string `function` member")
        );
    };
    let inputs = match arguments.get("inputs").and_then(Json::as_arr) {
        Some(items) if items.iter().all(|i| i.as_str().is_some()) => items.to_vec(),
        _ => {
            return Err(Fault::ArgumentsInvalid
                .refuse("`differentiate_llvm` needs an `inputs` array of parameter names"))
        }
    };
    let at = match arguments.get("at").and_then(Json::as_arr) {
        Some(items) if items.iter().all(|i| i.as_f64().is_some()) => items.to_vec(),
        _ => {
            return Err(Fault::ArgumentsInvalid
                .refuse("`differentiate_llvm` needs an `at` array of finite numbers"))
        }
    };
    let mut rebuilt = vec![
        ("module", s(module)),
        ("function", s(function)),
        ("inputs", Json::Arr(inputs)),
        ("at", Json::Arr(at)),
    ];
    match arguments.get("rules") {
        None => {}
        Some(rules) if rules.is_obj() => rebuilt.push(("rules", rules.clone())),
        Some(_) => {
            return Err(Fault::ArgumentsInvalid
                .refuse("the `rules` of `differentiate_llvm` is a rules document object"))
        }
    }
    Ok(obj(rebuilt))
}

fn tool_request(tool: &str, arguments: Json) -> Json {
    obj(vec![
        ("schema", s(tools::REQUEST_SCHEMA)),
        ("tool", s(tool)),
        ("arguments", arguments),
    ])
}

/// The `problem` argument, validated and rebuilt. Its own refusals -- syntax,
/// incomplete, unknown name, out of domain -- pass through with their codes.
fn validated_problem(arguments: &Json) -> Result<Json, Refused> {
    let Some(value) = arguments.get("problem") else {
        return Err(Fault::ArgumentsInvalid
            .refuse("the arguments have no `problem` member for this capability"));
    };
    Ok(problem::parse(&value.render())?.to_json())
}

/// The boundary's answer to a caller, in the protocol's own shape.
pub fn response(capability: &str, result: Json) -> Json {
    obj(vec![
        ("schema", s(RESPONSE_SCHEMA)),
        ("capability", s(capability)),
        ("ok", Json::Bool(true)),
        ("result", result),
    ])
}

/// The same, for a refusal: the code a caller matches on, with its detail and
/// its remedy, rather than a bare `ok:false` nobody can act on.
pub fn error_response(capability: &str, refused: &Refused) -> Json {
    obj(vec![
        ("schema", s(RESPONSE_SCHEMA)),
        ("capability", s(capability)),
        ("ok", Json::Bool(false)),
        (
            "error",
            obj(vec![
                ("code", s(&refused.code)),
                ("detail", s(&refused.detail)),
                ("remedy", s(&refused.remedy)),
            ]),
        ),
    ])
}

/// Read one `catalyst.adapter-response.v1` document back.
///
/// A boundary that only checks what it receives and never what it sends is
/// half a boundary; this is the other half, and the conformance fixtures
/// exercise it on documents this module built and on documents it refuses.
pub fn read_response(text: &str) -> Result<Response, Refused> {
    let document = read_object(text, Fault::ResponseInvalid)?;
    expect_schema(&document, RESPONSE_SCHEMA, Fault::ResponseInvalid)?;
    let Some(capability) = document.get("capability").and_then(Json::as_str) else {
        return Err(Fault::ResponseInvalid.refuse("the answer has no string `capability` member"));
    };
    if !CAPABILITIES.contains(&capability) {
        return Err(Fault::ResponseInvalid.refuse(format!(
            "the answer names `{}`, which is not a capability of this boundary",
            safe(capability)
        )));
    }
    let Some(ok) = document.get("ok").and_then(Json::as_bool) else {
        return Err(Fault::ResponseInvalid.refuse("the answer has no boolean `ok` member"));
    };
    if ok {
        if !document.get("result").is_some_and(Json::is_obj) {
            return Err(
                Fault::ResponseInvalid.refuse("an answer with ok:true needs a `result` object")
            );
        }
    } else {
        let Some(error) = document.get("error").filter(|value| value.is_obj()) else {
            return Err(
                Fault::ResponseInvalid.refuse("an answer with ok:false needs an `error` object")
            );
        };
        for member in ["code", "detail", "remedy"] {
            if error.get(member).and_then(Json::as_str).is_none() {
                return Err(Fault::ResponseInvalid.refuse(format!(
                    "the `error` of a refused answer has no string `{member}` member"
                )));
            }
        }
    }
    Ok(Response {
        capability: capability.to_owned(),
        ok,
    })
}

/// The boundary's capability discovery: what it offers, what it refuses, what
/// it is, and what it has not measured.
pub fn discovery() -> Json {
    obj(vec![
        ("schema", s(DISCOVERY_SCHEMA)),
        ("version", Json::Num(1.0)),
        ("label", s(LABEL)),
        ("inference", Json::Bool(false)),
        (
            "capabilities",
            Json::Arr(CAPABILITIES.iter().map(|name| capability(name)).collect()),
        ),
        (
            "errors",
            Json::Arr(FAULTS.iter().map(|f| f.describe()).collect()),
        ),
        ("continuation", continuation()),
    ])
}

fn capability(name: &str) -> Json {
    let arguments = match name {
        "discover" => "none",
        "export_go" => "a catalyst.problem.v1 `problem`, and a relative `out` path",
        "differentiate_llvm" => {
            "a string `module` of LLVM IR text, a string `function`, an `inputs` array of \
             parameter names, an `at` array of numbers, and optionally a `rules` document"
        }
        _ => "a catalyst.problem.v1 `problem`",
    };
    obj(vec![
        ("name", s(name)),
        ("arguments", s(arguments)),
        ("tool", s(name)),
        ("translates_to", s(tools::REQUEST_SCHEMA)),
    ])
}

/// The continuation rules of the boundary. They are the tool surface's own,
/// plus the one fact that is true here and nowhere else: translation is a pure
/// transformation, so a request may be translated again at any time without
/// anything having happened in between.
pub fn continuation() -> Json {
    let mut rules = vec![s(
        "Translation changes nothing and keeps nothing: the same request translates to the \
         same object every time, so a caller that lost its answer may translate again.",
    )];
    let shared = tools::discovery()
        .get("continuation")
        .and_then(|block| block.get("rules"))
        .and_then(Json::as_arr)
        .map(<[Json]>::to_vec)
        .unwrap_or_default();
    rules.extend(shared);
    obj(vec![
        ("rules", Json::Arr(rules)),
        ("resume", s("catalyst ai propose --state FILE --resume")),
    ])
}

fn read_object(text: &str, fault: Fault) -> Result<Json, Refused> {
    let document = Json::parse(text)
        .map_err(|error| fault.refuse(format!("at byte {}, expected {}", error.at, error.what)))?;
    if !document.is_obj() {
        return Err(fault.refuse("the document is not a JSON object"));
    }
    Ok(document)
}

fn expect_schema(document: &Json, wanted: &str, fault: Fault) -> Result<(), Refused> {
    match document.get("schema").and_then(Json::as_str) {
        Some(found) if found == wanted => Ok(()),
        Some(_) => Err(fault.refuse(format!("the `schema` member is not \"{wanted}\""))),
        None => Err(fault.refuse("the document has no string `schema` member")),
    }
}

/// A name out of a document, in a form a message can carry: no control
/// characters, and cut to a length that cannot flood a terminal.
pub fn safe(name: &str) -> String {
    let mut out: String = name.chars().filter(|c| !c.is_control()).take(32).collect();
    if name.chars().count() > 32 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remedy_names_every_capability_the_boundary_offers() {
        for name in CAPABILITIES {
            assert!(
                SUPPORTED.contains(name),
                "the remedy does not name `{name}`"
            );
        }
    }

    #[test]
    fn every_fault_maps_to_a_catalyst_code_with_a_remedy() {
        for fault in FAULTS {
            assert!(fault.code().starts_with("catalyst."));
            assert!(fault.meaning().len() >= 8);
            assert!(fault.remedy().len() >= 8);
        }
    }

    #[test]
    fn the_boundary_offers_exactly_the_tools_of_the_surface_behind_it() {
        assert_eq!(CAPABILITIES, tools::TOOLS);
    }
}
