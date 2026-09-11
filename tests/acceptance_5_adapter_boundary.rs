//! Outcome 5 — protocol and conformance checks at the local-adapter boundary
//! (criteria 5.1, 5.2, 5.4, and the label of 5.3 on every adapter surface).
//! No local runtime, no model, no process, no socket.
mod acceptance_common;
use acceptance_common as common;
use common::Json;

const FIXTURES: &[&str] = &[
    "request-valid",
    "request-malformed-refused",
    "response-valid",
    "response-malformed-refused",
    "discovery",
    "error-mapping",
    "continuation",
];

#[test]
fn oracle_5_1_conformance_fixtures_exercise_the_protocol_without_inference() {
    let dir = common::scratch("conformance");
    let run = common::catalyst(&["adapter", "conformance"], &dir, &[]);
    assert!(!run.panicked(), "{}", run.summary());
    assert_eq!(
        run.status,
        Some(0),
        "5.1: the conformance run passes:\n{}",
        run.summary()
    );
    let report = run.json();
    assert_eq!(
        report.str_field("schema"),
        "catalyst.adapter-conformance.v1"
    );
    assert_eq!(
        report.get("inference").and_then(Json::as_bool),
        Some(false),
        "5.1: no inference"
    );
    assert_eq!(
        report.str_field("label"),
        common::LABEL,
        "5.3: the report carries the label verbatim"
    );
    let fixtures = report
        .get("fixtures")
        .and_then(Json::as_arr)
        .expect("fixtures");
    for name in FIXTURES {
        let f = fixtures
            .iter()
            .find(|f| f.get("name").and_then(Json::as_str) == Some(name))
            .unwrap_or_else(|| {
                panic!(
                    "5.1: fixture `{name}` missing; have {:?}",
                    fixtures
                        .iter()
                        .map(|f| f.get("name").cloned())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            f.get("ok").and_then(Json::as_bool),
            Some(true),
            "5.1: fixture `{name}` fails: {f:?}"
        );
        assert!(
            f.str_field("detail").len() >= 8,
            "5.1: fixture `{name}` says what it checked"
        );
    }
    assert_eq!(report.num_field("failed"), 0.0);
    assert!(report.num_field("passed") >= FIXTURES.len() as f64);
    assert!(
        common::listing(&dir).is_empty(),
        "5.1: a conformance run writes nothing"
    );
}

#[test]
fn oracle_5_2_translation_yields_validated_requests_and_actionable_errors() {
    let dir = common::scratch("translate");
    let problem = common::simple_problem();
    let request = format!(
        r#"{{"schema":"catalyst.adapter-request.v1","capability":"evaluate","arguments":{{"problem":{problem}}}}}"#
    );
    common::write(&dir.join("request.json"), &request);
    let run = common::catalyst(
        &["adapter", "translate", "--request", "request.json"],
        &dir,
        &[],
    );
    let json = common::assert_ok(&run);
    let translated = json.get("request").expect("5.2: the translated request");
    assert_eq!(translated.str_field("schema"), "catalyst.tool-request.v1");
    assert_eq!(translated.str_field("tool"), "evaluate");
    assert!(
        translated.path("arguments.problem.function").is_some(),
        "5.2: the translated request carries the validated problem"
    );
    // The translated request is accepted by the tool surface itself.
    common::write(&dir.join("translated.json"), &translated.render());
    let call = common::catalyst(
        &["tools", "call", "--request", "translated.json"],
        &dir,
        &[],
    );
    let answer = common::assert_ok(&call);
    assert!(
        answer.path("result.value").and_then(Json::as_f64).is_some(),
        "5.2: the translated request evaluates:\n{}",
        call.summary()
    );

    for capability in ["image_generation", "shell", "local_inference", ""] {
        let request = format!(
            r#"{{"schema":"catalyst.adapter-request.v1","capability":{},"arguments":{{}}}}"#,
            common::quote(capability)
        );
        common::write(&dir.join("bad.json"), &request);
        let run = common::catalyst(
            &["adapter", "translate", "--request", "bad.json"],
            &dir,
            &[],
        );
        let json = common::assert_refusal(&run, "catalyst.adapter.capability_unsupported");
        let remedy = json.str_field("remedy");
        assert!(
            remedy.contains("evaluate") && remedy.contains("export_go"),
            "5.2: the remedy lists the supported capabilities:\n{}",
            run.summary()
        );
    }
    common::write(&dir.join("bad.json"), "{not json");
    let run = common::catalyst(
        &["adapter", "translate", "--request", "bad.json"],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.");
}

#[test]
fn oracle_5_3_the_label_is_on_every_adapter_surface() {
    let dir = common::scratch("label");
    let help = common::catalyst(&["adapter", "--help"], &dir, &[]);
    assert_eq!(help.status, Some(0), "{}", help.summary());
    assert!(
        help.stdout.contains(common::LABEL),
        "5.3: `catalyst adapter --help` carries the label verbatim:\n{}",
        help.summary()
    );
    let discover = common::catalyst(&["tools", "discover"], &dir, &[]);
    let json = common::assert_ok(&discover);
    assert_eq!(
        json.str_field("label"),
        common::LABEL,
        "5.3: discovery carries the label verbatim"
    );
}

#[test]
fn oracle_5_4_adapter_code_spawns_no_process_and_opens_no_socket() {
    let files: Vec<(String, String)> = common::src_files()
        .into_iter()
        .filter(|(p, _)| p.starts_with("src/adapter"))
        .collect();
    assert!(
        !files.is_empty(),
        "5.4: adapter code lives under src/adapter/ (or src/adapter.rs) so this oracle can read it"
    );
    let mut hits = Vec::new();
    for (path, text) in &files {
        for (number, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for needle in [
                "Command",
                "process::",
                "TcpStream",
                "TcpListener",
                "UdpSocket",
                "UnixStream",
                "UnixListener",
                "std::net",
                "os::unix::net",
                ".connect(",
                ".bind(",
                "spawn(",
            ] {
                if code.contains(needle) {
                    hits.push(format!("{path}:{} ({needle})", number + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "5.4: the adapter must spawn no process and open no socket; remove {hits:?}"
    );
    let lower: String = files.iter().map(|(_, t)| t.to_ascii_lowercase()).collect();
    for claim in ["ollama", "llama.cpp", "vllm", "lm studio"] {
        assert!(
            !lower.contains(claim),
            "5.5: no local runtime is named as supported in the adapter code (`{claim}`)"
        );
    }
}
