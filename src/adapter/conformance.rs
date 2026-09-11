//! The built-in conformance fixtures: seven checks over the protocol of
//! [`super::protocol`], run against documents this file carries.
//!
//! `docs/interface.md` §6. Every fixture is a comparison between two values
//! that this build computed, so the report is the same on every machine, needs
//! no model, needs no toolchain, needs no network and writes no file. That is
//! the claim `"inference":false` makes, and it is a claim about this code
//! rather than about anything that might be installed alongside it.
//!
//! # Why a failing fixture still exits 0
//!
//! Because the report *is* the answer. A conformance run that ended with a
//! refusal would tell a caller that the run failed without telling them which
//! fixture failed or what it saw; `failed` and the per-fixture `detail` carry
//! that, and a caller decides what to do about it.

use super::protocol::{self, Fault};
use crate::json::{obj, s, Json};
use crate::label::LABEL;
use crate::tools;

/// The problem every fixture uses. Small, exactly differentiable, and inside
/// its own domain, so no fixture depends on floating-point luck.
const PROBLEM: &str = "{\"schema\":\"catalyst.problem.v1\",\"name\":\"fixture\",\
                       \"goal\":\"a built-in conformance fixture\",\
                       \"function\":\"func fixture(x) = x * x\",\"inputs\":{\"x\":3},\
                       \"domains\":{\"x\":{\"min\":0,\"max\":9,\"unit\":\"\"}}}";

/// The module the `differentiate_llvm` fixture carries: the same function as
/// [`PROBLEM`], as a compiler would write it, so that the two lanes of the
/// boundary describe one computation.
const MODULE: &str = "define double @fixture(double %x) {\\nstart:\\n  %r = fmul double %x, %x\\n  ret double %r\\n}\\n";

/// The arguments of `differentiate_llvm` in every fixture that needs them.
fn differentiate_arguments() -> String {
    format!("{{\"module\":\"{MODULE}\",\"function\":\"fixture\",\"inputs\":[\"x\"],\"at\":[3]}}")
}

/// The names of the fixtures, in the order the report lists them.
pub const FIXTURES: &[&str] = &[
    "request-valid",
    "request-malformed-refused",
    "response-valid",
    "response-malformed-refused",
    "discovery",
    "error-mapping",
    "continuation",
];

/// What one fixture found: whether it held, and what it checked.
struct Outcome {
    ok: bool,
    detail: String,
}

fn held(detail: impl Into<String>) -> Outcome {
    Outcome {
        ok: true,
        detail: detail.into(),
    }
}

fn broke(detail: impl Into<String>) -> Outcome {
    Outcome {
        ok: false,
        detail: detail.into(),
    }
}

/// The whole report of §6.
pub fn report() -> Json {
    let outcomes: Vec<(&str, Outcome)> = FIXTURES.iter().map(|name| (*name, check(name))).collect();
    let passed = outcomes.iter().filter(|(_, o)| o.ok).count();
    let failed = outcomes.len() - passed;
    obj(vec![
        ("ok", Json::Bool(true)),
        ("schema", s(protocol::CONFORMANCE_SCHEMA)),
        ("label", s(LABEL)),
        ("inference", Json::Bool(false)),
        (
            "fixtures",
            Json::Arr(
                outcomes
                    .iter()
                    .map(|(name, outcome)| {
                        obj(vec![
                            ("name", s(name)),
                            ("ok", Json::Bool(outcome.ok)),
                            ("detail", s(&outcome.detail)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("passed", Json::Num(passed as f64)),
        ("failed", Json::Num(failed as f64)),
    ])
}

fn check(name: &str) -> Outcome {
    match name {
        "request-valid" => request_valid(),
        "request-malformed-refused" => request_malformed_refused(),
        "response-valid" => response_valid(),
        "response-malformed-refused" => response_malformed_refused(),
        "discovery" => discovery(),
        "error-mapping" => error_mapping(),
        "continuation" => continuation(),
        other => broke(format!("`{other}` is not a fixture of this build")),
    }
}

fn adapter_request(capability: &str, arguments: &str) -> String {
    format!(
        "{{\"schema\":\"{}\",\"capability\":\"{capability}\",\"arguments\":{arguments}}}",
        protocol::REQUEST_SCHEMA
    )
}

/// A well-formed request for every capability translates into a request the
/// tool surface names as one of its own.
fn request_valid() -> Outcome {
    for capability in protocol::CAPABILITIES {
        let arguments = match *capability {
            "discover" => "{}".to_owned(),
            "export_go" => format!("{{\"problem\":{PROBLEM},\"out\":\"fixture-export\"}}"),
            "differentiate_llvm" => differentiate_arguments(),
            _ => format!("{{\"problem\":{PROBLEM}}}"),
        };
        let request = match protocol::read_request(&adapter_request(capability, &arguments)) {
            Ok(request) => request,
            Err(refused) => {
                return broke(format!(
                    "a valid request for `{capability}` was refused as {}",
                    refused.code
                ))
            }
        };
        let translated = match protocol::translate(&request) {
            Ok(translated) => translated,
            Err(refused) => {
                return broke(format!(
                    "`{capability}` did not translate: {}",
                    refused.code
                ))
            }
        };
        if translated.get("schema").and_then(Json::as_str) != Some(tools::REQUEST_SCHEMA) {
            return broke(format!(
                "the translation of `{capability}` does not carry the tool-request schema"
            ));
        }
        if translated.get("tool").and_then(Json::as_str) != Some(*capability) {
            return broke(format!(
                "the translation of `{capability}` names another tool"
            ));
        }
        if !translated.get("arguments").is_some_and(Json::is_obj) {
            return broke(format!(
                "the translation of `{capability}` has no arguments object"
            ));
        }
    }
    held(format!(
        "each of the {} capabilities read as a {} and translated to a {} naming the same tool, \
         with a validated arguments object",
        protocol::CAPABILITIES.len(),
        protocol::REQUEST_SCHEMA,
        tools::REQUEST_SCHEMA
    ))
}

/// Every shape of malformed request is refused, and refused as the fault the
/// error mapping says it is.
fn request_malformed_refused() -> Outcome {
    let unsupported = protocol::CAPABILITIES.len();
    let cases: Vec<(&str, String, Fault)> = vec![
        ("not JSON", "{not json".to_owned(), Fault::RequestInvalid),
        ("an empty object", "{}".to_owned(), Fault::RequestInvalid),
        (
            "not an object",
            "[1, 2, 3]".to_owned(),
            Fault::RequestInvalid,
        ),
        (
            "the wrong schema",
            "{\"schema\":\"catalyst.other.v1\",\"capability\":\"evaluate\",\"arguments\":{}}"
                .to_owned(),
            Fault::RequestInvalid,
        ),
        (
            "no capability",
            format!(
                "{{\"schema\":\"{}\",\"arguments\":{{}}}}",
                protocol::REQUEST_SCHEMA
            ),
            Fault::RequestInvalid,
        ),
        (
            "arguments that are not an object",
            format!(
                "{{\"schema\":\"{}\",\"capability\":\"evaluate\",\"arguments\":5}}",
                protocol::REQUEST_SCHEMA
            ),
            Fault::RequestInvalid,
        ),
        (
            "an empty capability",
            adapter_request("", "{}"),
            Fault::CapabilityUnsupported,
        ),
        (
            "a capability this boundary does not offer",
            adapter_request("local_inference", "{}"),
            Fault::CapabilityUnsupported,
        ),
        (
            "a capability with no problem to work on",
            adapter_request("evaluate", "{}"),
            Fault::ArgumentsInvalid,
        ),
        (
            "an export with no destination",
            adapter_request("export_go", &format!("{{\"problem\":{PROBLEM}}}")),
            Fault::ArgumentsInvalid,
        ),
        (
            "a differentiation with no module",
            adapter_request(
                "differentiate_llvm",
                "{\"function\":\"fixture\",\"inputs\":[\"x\"],\"at\":[3]}",
            ),
            Fault::ArgumentsInvalid,
        ),
        (
            "a request that would approve its own result",
            adapter_request("evaluate", "{\"approve_own_result\":true}"),
            Fault::Authority,
        ),
    ];
    let total = cases.len();
    for (label, text, wanted) in cases {
        let outcome =
            protocol::read_request(&text).and_then(|request| protocol::translate(&request));
        match outcome {
            Ok(_) => return broke(format!("a request with {label} was accepted")),
            Err(refused) if refused.code != wanted.code() => {
                return broke(format!(
                    "a request with {label} was refused as {} rather than {}",
                    refused.code,
                    wanted.code()
                ))
            }
            Err(refused) if refused.remedy.len() < 8 => {
                return broke(format!("the refusal of {label} carries no remedy"))
            }
            Err(_) => {}
        }
    }
    held(format!(
        "{total} malformed or refused requests -- including {unsupported} unsupported \
         capabilities' worth of names, absent arguments and an attempt to approve its own \
         result -- each produced the mapped code with a remedy"
    ))
}

/// Answers the boundary builds read back as the answers they are.
fn response_valid() -> Outcome {
    let ok = protocol::response("evaluate", obj(vec![("value", Json::Num(9.0))]));
    match protocol::read_response(&ok.render()) {
        Ok(answer) if answer.ok && answer.capability == "evaluate" => {}
        Ok(_) => return broke("a result answer read back as something else"),
        Err(refused) => {
            return broke(format!(
                "a valid result answer was refused as {}",
                refused.code
            ))
        }
    }
    let refusal = Fault::CapabilityUnsupported.refuse("a fixture refusal");
    let refused_answer = protocol::error_response("evaluate", &refusal);
    match protocol::read_response(&refused_answer.render()) {
        Ok(answer) if !answer.ok => held(format!(
            "a {} carrying a result, and one carrying code, detail and remedy, both read back \
             with the capability and the verdict they were built with",
            protocol::RESPONSE_SCHEMA
        )),
        Ok(_) => broke("a refusal answer read back as a result"),
        Err(refused) => broke(format!(
            "a valid refusal answer was refused as {}",
            refused.code
        )),
    }
}

/// Every shape of malformed answer is refused.
fn response_malformed_refused() -> Outcome {
    let schema = protocol::RESPONSE_SCHEMA;
    let cases: Vec<(&str, String)> = vec![
        ("not JSON", "}{".to_owned()),
        (
            "the wrong schema",
            "{\"schema\":\"catalyst.other.v1\",\"capability\":\"evaluate\",\"ok\":true,\"result\":{}}"
                .to_owned(),
        ),
        ("no capability", format!("{{\"schema\":\"{schema}\",\"ok\":true,\"result\":{{}}}}")),
        (
            "a capability this boundary does not offer",
            format!("{{\"schema\":\"{schema}\",\"capability\":\"shell\",\"ok\":true,\"result\":{{}}}}"),
        ),
        ("no verdict", format!("{{\"schema\":\"{schema}\",\"capability\":\"evaluate\",\"result\":{{}}}}")),
        (
            "a verdict of true and no result",
            format!("{{\"schema\":\"{schema}\",\"capability\":\"evaluate\",\"ok\":true}}"),
        ),
        (
            "a verdict of false and no error",
            format!("{{\"schema\":\"{schema}\",\"capability\":\"evaluate\",\"ok\":false}}"),
        ),
        (
            "an error with no remedy",
            format!("{{\"schema\":\"{schema}\",\"capability\":\"evaluate\",\"ok\":false,\"error\":{{\"code\":\"catalyst.syntax\",\"detail\":\"something\"}}}}"),
        ),
    ];
    let total = cases.len();
    for (label, text) in cases {
        match protocol::read_response(&text) {
            Ok(_) => return broke(format!("an answer with {label} was accepted")),
            Err(refused) if refused.code != Fault::ResponseInvalid.code() => {
                return broke(format!(
                    "an answer with {label} was refused as {} rather than {}",
                    refused.code,
                    Fault::ResponseInvalid.code()
                ))
            }
            Err(_) => {}
        }
    }
    held(format!(
        "{total} malformed answers -- a bad schema, an unknown capability, a missing verdict, \
         a result that is not there and an error without a remedy -- were each refused as {}",
        Fault::ResponseInvalid.code()
    ))
}

/// The boundary's discovery and the tool surface's discovery agree, and both
/// carry the label.
fn discovery() -> Outcome {
    let mine = protocol::discovery();
    let surface = tools::discovery();
    if mine.get("label").and_then(Json::as_str) != Some(LABEL) {
        return broke("the boundary's discovery does not carry the label");
    }
    if mine.get("inference").and_then(Json::as_bool) != Some(false) {
        return broke("the boundary's discovery does not say that it runs no inference");
    }
    if surface.get("label").and_then(Json::as_str) != Some(LABEL) {
        return broke("the tool surface's discovery does not carry the same label");
    }
    let listed: Vec<&str> = mine
        .get("capabilities")
        .and_then(Json::as_arr)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Json::as_str))
                .collect()
        })
        .unwrap_or_default();
    if listed != protocol::CAPABILITIES {
        return broke("the discovery document does not list every capability, in order");
    }
    let served: Vec<&str> = surface
        .get("tools")
        .and_then(Json::as_arr)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Json::as_str))
                .collect()
        })
        .unwrap_or_default();
    if served != protocol::CAPABILITIES {
        return broke("a capability is offered that the tool surface behind it does not serve");
    }
    held(format!(
        "the boundary lists {} capabilities, the tool surface serves the same {} by name, \
         both documents carry the label verbatim, and both declare that no inference happens",
        listed.len(),
        served.len()
    ))
}

/// Every fault maps to a `catalyst.` code with a meaning and a remedy, and the
/// document publishes the whole map.
fn error_mapping() -> Outcome {
    let published = protocol::discovery()
        .get("errors")
        .and_then(Json::as_arr)
        .map(<[Json]>::to_vec)
        .unwrap_or_default();
    if published.len() != protocol::FAULTS.len() {
        return broke("the discovery document does not publish every fault of the boundary");
    }
    for fault in protocol::FAULTS {
        if !fault.code().starts_with("catalyst.") {
            return broke(format!(
                "`{}` does not map to a catalyst code",
                fault.name()
            ));
        }
        if fault.meaning().len() < 8 || fault.remedy().len() < 8 {
            return broke(format!(
                "`{}` maps to a code with no meaning or no remedy",
                fault.name()
            ));
        }
        let found = published
            .iter()
            .any(|entry| entry.get("code").and_then(Json::as_str) == Some(fault.code()));
        if !found {
            return broke(format!("`{}` is not in the published map", fault.code()));
        }
        let refused = fault.refuse("a fixture detail");
        if refused.code != fault.code() || refused.remedy != fault.remedy() {
            return broke(format!(
                "`{}` refuses with a code or a remedy other than the one it publishes",
                fault.name()
            ));
        }
    }
    let unsupported = Fault::CapabilityUnsupported.remedy();
    for name in protocol::CAPABILITIES {
        if !unsupported.contains(name) {
            return broke(format!(
                "the remedy for an unsupported capability does not name `{name}`"
            ));
        }
    }
    held(format!(
        "each of the {} faults maps to a catalyst code with a meaning and a remedy, refuses \
         with exactly that code and remedy, and appears in the published map; the unsupported \
         remedy names all {} capabilities",
        protocol::FAULTS.len(),
        protocol::CAPABILITIES.len()
    ))
}

/// The continuation rules are there, and they name how to pick work up.
fn continuation() -> Outcome {
    let block = protocol::continuation();
    let rules = block
        .get("rules")
        .and_then(Json::as_arr)
        .map(<[Json]>::to_vec)
        .unwrap_or_default();
    if rules.is_empty() {
        return broke("the boundary declares no continuation rules");
    }
    if rules
        .iter()
        .any(|rule| rule.as_str().unwrap_or("").len() < 8)
    {
        return broke("a continuation rule says nothing");
    }
    let resume = block
        .get("resume")
        .and_then(Json::as_str)
        .unwrap_or_default();
    if !resume.contains("--resume") {
        return broke("the continuation does not name the command that resumes saved work");
    }
    // Translating the same request twice gives the same object: the rule that
    // says a caller may translate again is checked, not merely stated.
    let text = adapter_request("evaluate", &format!("{{\"problem\":{PROBLEM}}}"));
    let once = protocol::read_request(&text).and_then(|r| protocol::translate(&r));
    let twice = protocol::read_request(&text).and_then(|r| protocol::translate(&r));
    match (once, twice) {
        (Ok(first), Ok(second)) if first == second => held(format!(
            "{} continuation rules, each one saying something, a resume line naming --resume, \
             and one request translating to the same object twice over",
            rules.len()
        )),
        (Ok(_), Ok(_)) => broke("the same request translated to two different objects"),
        _ => broke("a request that translates once did not translate again"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_holds_and_the_report_says_so() {
        let report = report();
        assert_eq!(report.get("failed").and_then(Json::as_f64), Some(0.0));
        let fixtures = report
            .get("fixtures")
            .and_then(Json::as_arr)
            .expect("fixtures")
            .to_vec();
        assert_eq!(fixtures.len(), FIXTURES.len());
        for fixture in &fixtures {
            assert_eq!(
                fixture.get("ok").and_then(Json::as_bool),
                Some(true),
                "{fixture:?}"
            );
            assert!(
                fixture
                    .get("detail")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .len()
                    >= 8
            );
        }
    }

    #[test]
    fn the_report_carries_the_label_and_claims_no_inference() {
        let report = report();
        assert_eq!(report.get("label").and_then(Json::as_str), Some(LABEL));
        assert_eq!(report.get("inference").and_then(Json::as_bool), Some(false));
    }

    #[test]
    fn a_fixture_this_build_does_not_have_is_reported_rather_than_ignored() {
        assert!(!check("no-such-fixture").ok);
    }
}
