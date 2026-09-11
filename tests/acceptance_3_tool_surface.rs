//! Outcome 3 — frontier-agent operation through Catalyst's validated tool
//! surface (criteria 3.1–3.6) and the credential rule C14 in its stronger
//! reading. The AI command is a stub written by this oracle: it records the
//! request it received and answers with a canned reply, so no inference and
//! no network are involved. The live call (3.7) is a camera fact.
mod acceptance_common;
use acceptance_common as common;
use common::Json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const SENTINEL_KEY: &str = "SENTINEL-C14-KEY-3b9d2ac5-never-printed";
const SENTINEL_TOKEN: &str = "SENTINEL-3-5-TOKEN-7d031b0c-never-sent";
const UNAPPROVED: &str = "UNAPPROVED-FIELD-CONTENT-e5fa9522";

const STUB: &str = r#"#!/bin/sh
d="$CATALYST_STUB_DIR"
cat > "$d/stdin.txt"
printf '%s\n' "$@" > "$d/argv.txt"
if [ "$CATALYST_STUB_FAIL_ONCE" = "1" ] && [ ! -f "$d/failed-once" ]; then
  : > "$d/failed-once"
  exit 130
fi
cat "$d/reply.json"
"#;

/// Install the stub AI command under `dir`, with the reply it should give.
fn stub(dir: &Path, reply_text: &str) -> (PathBuf, PathBuf) {
    let bin = dir.join("stub-bin");
    let capture = dir.join("stub-capture");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&capture).unwrap();
    let path = bin.join("claude");
    common::write(&path, STUB);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let envelope = format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"result":{}}}"#,
        common::quote(reply_text)
    );
    common::write(&capture.join("reply.json"), &envelope);
    (path, capture)
}

fn proposal_reply() -> String {
    format!(
        "Here is a structured problem for that goal:\n\n```json\n{}\n```\n",
        common::spring_problem()
    )
}

fn tool_request(tool: &str, arguments: &str) -> String {
    format!(
        r#"{{"schema":"catalyst.tool-request.v1","tool":{},"arguments":{arguments}}}"#,
        common::quote(tool)
    )
}

#[test]
fn oracle_3_1_capability_discovery_is_versioned_and_complete() {
    let dir = common::scratch("discover");
    let run = common::catalyst(&["tools", "discover"], &dir, &[]);
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("schema"), "catalyst.tools.v1");
    assert_eq!(json.num_field("version"), 1.0);
    assert_eq!(json.str_field("label"), common::LABEL);
    let tools = json
        .get("tools")
        .and_then(Json::as_arr)
        .expect("3.1: tools");
    for name in ["validate_problem", "evaluate", "export_go"] {
        let t = tools
            .iter()
            .find(|t| t.get("name").and_then(Json::as_str) == Some(name))
            .unwrap_or_else(|| panic!("3.1: tool `{name}` missing"));
        assert!(t.str_field("description").len() >= 8);
        assert!(
            t.get("request_schema").and_then(Json::as_obj).is_some(),
            "3.1: `{name}` has a request schema"
        );
        assert!(
            t.get("response_schema").and_then(Json::as_obj).is_some(),
            "3.1: `{name}` has a response schema"
        );
    }
    let errors = json
        .get("errors")
        .and_then(Json::as_arr)
        .expect("3.1: errors");
    assert!(errors.len() >= 3);
    for e in errors {
        assert!(e.str_field("code").starts_with("catalyst."));
        assert!(
            e.str_field("meaning").len() >= 8 && e.str_field("remedy").len() >= 8,
            "3.1: every error has a meaning and a remedy: {e:?}"
        );
    }
    let rules = json
        .path("continuation.rules")
        .and_then(Json::as_arr)
        .expect("3.1: continuation rules");
    assert!(!rules.is_empty());
    assert!(
        json.path("continuation.resume")
            .and_then(Json::as_str)
            .is_some_and(|s| s.contains("--resume")),
        "3.1: continuation names the resume command"
    );
}

#[test]
fn oracle_3_2_a_malformed_proposal_is_refused_with_a_structured_error_and_changes_no_state() {
    let dir = common::scratch("malformed");
    let before = common::listing(&dir);
    let broken_problem = r#"{"schema":"catalyst.problem.v1","name":"x"}"#;
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "not JSON",
            "this is not json".to_string(),
            "catalyst.tool_request_invalid",
        ),
        (
            "empty object",
            "{}".to_string(),
            "catalyst.tool_request_invalid",
        ),
        (
            "wrong schema",
            r#"{"schema":"catalyst.other.v1","tool":"evaluate","arguments":{}}"#.to_string(),
            "catalyst.tool_request_invalid",
        ),
        (
            "unknown tool",
            tool_request("nonexistent", "{}"),
            "catalyst.tool_unknown",
        ),
        (
            "arguments not an object",
            tool_request("evaluate", "5"),
            "catalyst.tool_request_invalid",
        ),
        (
            "incomplete problem",
            tool_request("evaluate", &format!(r#"{{"problem":{broken_problem}}}"#)),
            "catalyst.",
        ),
        (
            "export with a broken problem",
            tool_request(
                "export_go",
                &format!(r#"{{"problem":{broken_problem},"out":"never-created"}}"#),
            ),
            "catalyst.",
        ),
        (
            "export escaping the workspace",
            tool_request(
                "export_go",
                &format!(
                    r#"{{"problem":{},"out":"../never-created"}}"#,
                    common::simple_problem()
                ),
            ),
            "catalyst.path_refused",
        ),
    ];
    for (label, request, code) in cases {
        let run = common::catalyst_stdin(&["tools", "call", "--request", "-"], &dir, &request);
        let json = common::assert_refusal(&run, code);
        assert!(
            json.str_field("detail").len() >= 8,
            "3.2: `{label}` refusal says what was wrong:\n{}",
            run.summary()
        );
    }
    assert_eq!(
        common::listing(&dir),
        before,
        "3.2: a refused request changes no state"
    );
    assert!(
        !dir.join("never-created").exists()
            && !dir.parent().unwrap().join("never-created").exists()
    );
}

#[test]
fn oracle_3_2b_valid_requests_are_served() {
    let dir = common::scratch("valid-calls");
    let request = tool_request(
        "validate_problem",
        &format!(r#"{{"problem":{}}}"#, common::simple_problem()),
    );
    let run = common::catalyst_stdin(&["tools", "call", "--request", "-"], &dir, &request);
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("schema"), "catalyst.tool-response.v1");
    assert_eq!(json.str_field("tool"), "validate_problem");
    let request = tool_request(
        "evaluate",
        &format!(r#"{{"problem":{}}}"#, common::simple_problem()),
    );
    let run = common::catalyst_stdin(&["tools", "call", "--request", "-"], &dir, &request);
    let json = common::assert_ok(&run);
    let value = json
        .path("result.value")
        .and_then(Json::as_f64)
        .expect("result.value");
    assert!(common::within(
        value,
        0.7 * 1.3 + 0.7f64.sin(),
        1e-12,
        1e-12
    ));
    common::write(
        &dir.join("req.json"),
        &tool_request(
            "export_go",
            &format!(
                r#"{{"problem":{},"out":"tool-export"}}"#,
                common::simple_problem()
            ),
        ),
    );
    let run = common::catalyst(&["tools", "call", "--request", "req.json"], &dir, &[]);
    common::assert_ok(&run);
    assert!(
        dir.join("tool-export/function.go").is_file(),
        "3.x: export_go through the tool surface writes the export"
    );
}

#[test]
fn oracle_3_3_a_request_to_change_acceptance_tolerance_or_approve_itself_is_refused() {
    let dir = common::scratch("authority");
    let problem = common::simple_problem();
    for tool in [
        "set_acceptance",
        "set_tolerance",
        "approve_result",
        "approve",
    ] {
        let run = common::catalyst_stdin(
            &["tools", "call", "--request", "-"],
            &dir,
            &tool_request(tool, "{}"),
        );
        common::assert_refusal(&run, "catalyst.authority_refused");
    }
    for arguments in [
        format!(r#"{{"problem":{problem},"tolerance":{{"relative":1.0}}}}"#),
        format!(r#"{{"problem":{problem},"acceptance":"anything goes"}}"#),
        format!(r#"{{"problem":{problem},"approve_own_result":true}}"#),
    ] {
        let run = common::catalyst_stdin(
            &["tools", "call", "--request", "-"],
            &dir,
            &tool_request("evaluate", &arguments),
        );
        common::assert_refusal(&run, "catalyst.authority_refused");
    }
    assert!(
        common::listing(&dir).is_empty(),
        "3.3: refused requests change nothing"
    );
}

#[test]
fn oracle_3_4_cancellation_and_context_renewal_leave_saved_work_resumable() {
    let dir = common::scratch("resume");
    let (stub_path, capture) = stub(&dir, &proposal_reply());
    let env = [
        ("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned()),
        ("CATALYST_STUB_FAIL_ONCE", "1".to_string()),
    ];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let stub_arg = stub_path.to_string_lossy().into_owned();
    let args = [
        "ai",
        "propose",
        "--goal",
        "a spring that settles quickly",
        "--out",
        "proposed.json",
        "--state",
        "ai-state.json",
        "--command",
        &stub_arg,
    ];
    // First call: the command is interrupted (exit 130).
    let run = common::catalyst(&args, &dir, &pairs);
    let json = common::assert_refusal(&run, "catalyst.ai_call_failed");
    assert!(
        json.str_field("remedy").contains("--resume"),
        "3.4: the remedy says how to resume:\n{}",
        run.summary()
    );
    let state = Json::parse(&common::read_file(&dir.join("ai-state.json")))
        .expect("3.4: the state file is valid JSON after an interrupted call");
    assert_eq!(state.str_field("schema"), "catalyst.ai-state.v1");
    assert_eq!(state.get("pending").and_then(Json::as_bool), Some(true));
    assert_eq!(
        state.get("attempts").and_then(Json::as_arr).map(Vec::len),
        Some(1)
    );
    assert!(
        !dir.join("proposed.json").exists(),
        "3.4: no proposal was written by a failed call"
    );
    // Running again without --resume is refused: the pending work is not silently discarded.
    let run = common::catalyst(&args, &dir, &pairs);
    common::assert_refusal(&run, "catalyst.ai_pending");
    // Resume: a fresh context, the same saved work.
    let mut resumed = args.to_vec();
    resumed.push("--resume");
    let run = common::catalyst(&resumed, &dir, &pairs);
    let json = common::assert_ok(&run);
    assert_eq!(json.get("validated").and_then(Json::as_bool), Some(true));
    let proposed = Json::parse(&common::read_file(&dir.join("proposed.json")))
        .expect("3.4: the accepted proposal is written");
    assert_eq!(proposed.str_field("schema"), "catalyst.problem.v1");
    let state = Json::parse(&common::read_file(&dir.join("ai-state.json"))).unwrap();
    assert_eq!(state.get("pending").and_then(Json::as_bool), Some(false));
    assert_eq!(
        state.get("attempts").and_then(Json::as_arr).map(Vec::len),
        Some(2)
    );
}

#[test]
fn oracle_3_5_and_c14_a_planted_secret_reaches_no_request_payload_and_no_log_line() {
    let dir = common::scratch("secret");
    let (stub_path, capture) = stub(&dir, &proposal_reply());
    let env = [
        ("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned()),
        ("ANTHROPIC_API_KEY", SENTINEL_KEY.to_string()),
        ("CATALYST_AI_TOKEN", SENTINEL_TOKEN.to_string()),
    ];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let stub_arg = stub_path.to_string_lossy().into_owned();
    let run = common::catalyst(
        &[
            "ai",
            "propose",
            "--goal",
            "a spring that settles quickly",
            "--out",
            "proposed.json",
            "--state",
            "ai-state.json",
            "--transcript",
            "transcript.txt",
            "--command",
            &stub_arg,
        ],
        &dir,
        &pairs,
    );
    common::assert_ok(&run);
    let stdin = common::read_file(&capture.join("stdin.txt"));
    let argv = common::read_file(&capture.join("argv.txt"));
    assert!(
        stdin.contains("a spring that settles quickly"),
        "3.6: the approved goal is in the request"
    );
    assert_eq!(
        argv.lines().collect::<Vec<_>>(),
        vec!["-p", "--model", "claude-opus-5", "--output-format", "json"],
        "the command is run with exactly the agreed arguments"
    );
    let mut everything = vec![
        ("request stdin", stdin),
        ("request argv", argv),
        ("stdout", run.stdout.clone()),
        ("stderr", run.stderr.clone()),
    ];
    for file in ["transcript.txt", "ai-state.json", "proposed.json"] {
        everything.push((file, common::read_file(&dir.join(file))));
    }
    for (where_, text) in &everything {
        assert!(
            !text.contains(SENTINEL_KEY),
            "3.5/C14: the credential from the environment appears in {where_}"
        );
        assert!(
            !text.contains(SENTINEL_TOKEN),
            "3.5: the planted secret appears in {where_}"
        );
    }
    let transcript = &everything[4].1;
    assert!(
        transcript.contains("a spring that settles quickly")
            && transcript.contains("catalyst.problem.v1"),
        "3.7 support: the transcript records the request and the response"
    );
    // With the command absent the feature says so; the credential still never shows.
    let run = common::catalyst(
        &[
            "ai",
            "propose",
            "--goal",
            "anything",
            "--out",
            "p2.json",
            "--state",
            "s2.json",
            "--command",
            "/nonexistent/claude",
        ],
        &dir,
        &pairs,
    );
    let json = common::assert_refusal(&run, "catalyst.ai_unavailable");
    assert!(!run.stdout.contains(SENTINEL_KEY) && !run.stderr.contains(SENTINEL_KEY));
    assert!(json.str_field("remedy").len() >= 8);
}

#[test]
fn oracle_3_6_only_explicitly_approved_data_is_placed_in_an_outgoing_request() {
    let dir = common::scratch("approved");
    let (stub_path, capture) = stub(&dir, &proposal_reply());
    let env = [("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned())];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut context = Json::parse(&common::simple_problem()).unwrap();
    if let Json::Obj(pairs_) = &mut context {
        pairs_.push((
            "private".to_string(),
            Json::Obj(vec![(
                "note".to_string(),
                Json::Str(format!("{UNAPPROVED}-private")),
            )]),
        ));
        pairs_.push((
            "scratch".to_string(),
            Json::Str(format!("{UNAPPROVED}-scratch")),
        ));
    }
    common::write(&dir.join("context.json"), &context.render());
    let stub_arg = stub_path.to_string_lossy().into_owned();
    let run = common::catalyst(
        &[
            "ai",
            "propose",
            "--goal",
            "refine the product goal",
            "--context",
            "context.json",
            "--out",
            "proposed.json",
            "--state",
            "ai-state.json",
            "--command",
            &stub_arg,
        ],
        &dir,
        &pairs,
    );
    common::assert_ok(&run);
    let stdin = common::read_file(&capture.join("stdin.txt"));
    assert!(
        stdin.contains("refine the product goal"),
        "3.6: the approved goal is sent"
    );
    assert!(
        stdin.contains("x * y + sin(x)"),
        "3.6: the approved function of the context is sent"
    );
    assert!(
        !stdin.contains(UNAPPROVED),
        "3.6: an unapproved field of the context must never be placed in the request:\n{stdin}"
    );
}

#[test]
fn oracle_c14_no_source_reads_a_credential_from_a_file_in_the_tree() {
    let mut hits = Vec::new();
    for (path, text) in common::src_files() {
        for (number, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("").to_ascii_lowercase();
            for needle in [
                "\".env\"",
                "\".netrc\"",
                "\"credentials\"",
                "\".anthropic\"",
                "\"secrets\"",
                "api-key.txt",
                "token.txt",
            ] {
                if code.contains(needle) {
                    hits.push(format!("{path}:{} ({needle})", number + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "C14: the AI path must not read a credential from a file in the tree: {hits:?}"
    );
}
