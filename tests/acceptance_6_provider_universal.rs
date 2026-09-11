//! Provider universality (§7). Catalyst is bought by nobody: it holds no
//! credential, has no account and makes no provider network call. It runs a
//! command-line program the user has already signed into with whatever plan
//! they pay for -- a Claude Max seat through `claude`, a Codex Pro seat
//! through `codex`, any other signed-in CLI through its own argument shape --
//! and reads that program's answer.
//!
//! Every AI command in this file is a stub written by the oracle. It records
//! what it was given and answers with a canned reply, so no inference, no
//! network and no subscription are involved in any check here.
mod acceptance_common;
use acceptance_common as common;
use common::Json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const SENTINEL_KEY: &str = "SENTINEL-6-KEY-4f11c9a2-never-read";
const SENTINEL_TOKEN: &str = "SENTINEL-6-TOKEN-9c02de77-never-sent";

/// Records argv and stdin, then prints the reply file verbatim.
const STUB: &str = r#"#!/bin/sh
d="$CATALYST_STUB_DIR"
cat > "$d/stdin.txt"
printf '%s\n' "$@" > "$d/argv.txt"
cat "$d/reply.json"
"#;

/// Install a stub command under `dir` with the given name, answering with
/// `reply` exactly as written (the caller decides whether it is an envelope).
fn stub_named(dir: &Path, name: &str, reply: &str) -> (PathBuf, PathBuf) {
    let bin = dir.join("stub-bin");
    let capture = dir.join("stub-capture");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&capture).unwrap();
    let path = bin.join(name);
    common::write(&path, STUB);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    common::write(&capture.join("reply.json"), reply);
    (path, capture)
}

/// The answer text a cooperating model gives: a fenced problem object.
fn answer_text() -> String {
    format!(
        "Here is a structured problem for that goal:\n\n```json\n{}\n```\n",
        common::spring_problem()
    )
}

/// That answer wrapped in the Claude CLI's JSON envelope.
fn envelope_reply() -> String {
    format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"result":{}}}"#,
        common::quote(&answer_text())
    )
}

/// The arguments a stub actually received, one per line.
fn argv_of(capture: &Path) -> Vec<String> {
    common::read_file(&capture.join("argv.txt"))
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Ask for a proposal from `command`, with whatever extra flags are given.
fn propose(dir: &Path, command: &str, extra: &[&str], env: &[(&str, &str)]) -> common::Run {
    let mut args = vec![
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
        command,
    ];
    args.extend_from_slice(extra);
    common::catalyst(&args, dir, env)
}

#[test]
fn oracle_6_1_the_default_invocation_is_unchanged() {
    // The agreed default argument list is the contract every earlier oracle
    // was bound to. Making Catalyst universal must not move it.
    let dir = common::scratch("universal-default");
    let (path, capture) = stub_named(&dir, "claude", &envelope_reply());
    let env = [("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned())];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(&dir, &command, &[], &pairs);
    let json = common::assert_ok(&run);
    assert_eq!(json.get("validated").and_then(Json::as_bool), Some(true));
    assert_eq!(
        argv_of(&capture),
        vec!["-p", "--model", "claude-opus-5", "--output-format", "json"],
        "the default argument list is exactly the one already agreed"
    );
}

#[test]
fn oracle_6_2_another_subscription_cli_shape_is_expressible() {
    // A different plan's CLI takes different arguments. Expressing that is all
    // it takes to use Catalyst with it -- no code change, no wrapper script.
    let dir = common::scratch("universal-shape");
    let (path, capture) = stub_named(&dir, "codex", &envelope_reply());
    let env = [("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned())];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(
        &dir,
        &command,
        &["--provider-args", "exec --model {model} --json"],
        &pairs,
    );
    let json = common::assert_ok(&run);
    assert_eq!(json.get("validated").and_then(Json::as_bool), Some(true));
    assert_eq!(
        argv_of(&capture),
        vec!["exec", "--model", "claude-opus-5", "--json"],
        "the template is the argument list, with {{model}} resolved"
    );
    let proposed = Json::parse(&common::read_file(&dir.join("proposed.json")))
        .expect("the accepted proposal is written");
    assert_eq!(proposed.str_field("schema"), "catalyst.problem.v1");
}

#[test]
fn oracle_6_3_the_environment_can_carry_the_shape_too() {
    // A user configures their plan once, in their shell, not on every command.
    let dir = common::scratch("universal-env");
    let (path, capture) = stub_named(&dir, "plan-cli", &envelope_reply());
    let env = [
        ("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned()),
        ("CATALYST_AI_ARGS", "run --quiet --as {model}".to_string()),
        ("CATALYST_AI_MODEL", "some-other-model".to_string()),
    ];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(&dir, &command, &[], &pairs);
    common::assert_ok(&run);
    assert_eq!(
        argv_of(&capture),
        vec!["run", "--quiet", "--as", "some-other-model"],
        "the environment supplies the shape and the model"
    );
}

#[test]
fn oracle_6_4_a_template_without_a_model_runs_without_one() {
    // Some signed-in CLIs take no model argument at all. Naming no {model}
    // must mean exactly that, not an empty argument.
    let dir = common::scratch("universal-nomodel");
    let (path, capture) = stub_named(&dir, "modelless", &envelope_reply());
    let env = [("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned())];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(
        &dir,
        &command,
        &["--provider-args", "--prompt-stdin"],
        &pairs,
    );
    common::assert_ok(&run);
    assert_eq!(argv_of(&capture), vec!["--prompt-stdin"]);
    // An empty template names no command shape at all and is refused.
    let dir = common::scratch("universal-empty");
    let (path, capture) = stub_named(&dir, "modelless", &envelope_reply());
    let env = [("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned())];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(&dir, &command, &["--provider-args", "   "], &pairs);
    common::assert_refusal(&run, "catalyst.ai_arguments_invalid");
}

#[test]
fn oracle_6_5_a_plain_answer_without_an_envelope_is_accepted() {
    // Not every CLI wraps its answer. One that simply prints it is usable.
    let dir = common::scratch("universal-plain");
    let (path, capture) = stub_named(&dir, "plain", &answer_text());
    let env = [("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned())];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(&dir, &command, &["--provider-args", "--plain"], &pairs);
    let json = common::assert_ok(&run);
    assert_eq!(json.get("validated").and_then(Json::as_bool), Some(true));
    let proposed = Json::parse(&common::read_file(&dir.join("proposed.json")))
        .expect("the plain answer is validated and written");
    assert_eq!(proposed.str_field("schema"), "catalyst.problem.v1");
}

#[test]
fn oracle_6_6_status_names_the_exact_arguments_it_would_use() {
    // A user must be able to see what Catalyst will run before it runs it.
    let dir = common::scratch("universal-status");
    let (path, _capture) = stub_named(&dir, "shown", &envelope_reply());
    let command = path.to_string_lossy().into_owned();
    let run = common::catalyst(
        &[
            "ai",
            "status",
            "--command",
            &command,
            "--provider-args",
            "exec --model {model} --json",
            "--model",
            "a-plan-model",
        ],
        &dir,
        &[],
    );
    let json = common::assert_ok(&run);
    assert_eq!(json.get("available").and_then(Json::as_bool), Some(true));
    assert_eq!(json.str_field("model"), "a-plan-model");
    let arguments: Vec<String> = json
        .get("arguments")
        .and_then(Json::as_arr)
        .expect("status names the argument list")
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        arguments,
        vec!["exec", "--model", "a-plan-model", "--json"],
        "status shows the argument list the command would be run with"
    );
    // The flag wins over the environment for the model, as everywhere else.
    let run = common::catalyst(
        &[
            "ai",
            "status",
            "--command",
            &command,
            "--model",
            "flag-wins",
        ],
        &dir,
        &[("CATALYST_AI_MODEL", "env-loses")],
    );
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("model"), "flag-wins");
}

#[test]
fn oracle_6_7_no_credential_is_read_or_forwarded_on_the_universal_path() {
    // C14 in its stronger reading, on the new path: a plan's credential lives
    // in the CLI the user signed into, never in Catalyst.
    let dir = common::scratch("universal-secret");
    let (path, capture) = stub_named(&dir, "codex", &envelope_reply());
    let env = [
        ("CATALYST_STUB_DIR", capture.to_string_lossy().into_owned()),
        ("ANTHROPIC_API_KEY", SENTINEL_KEY.to_string()),
        ("OPENAI_API_KEY", SENTINEL_KEY.to_string()),
        ("CATALYST_AI_TOKEN", SENTINEL_TOKEN.to_string()),
    ];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let command = path.to_string_lossy().into_owned();
    let run = propose(
        &dir,
        &command,
        &["--provider-args", "exec --model {model} --json"],
        &pairs,
    );
    common::assert_ok(&run);
    let mut everything = vec![
        (
            "the request stdin",
            common::read_file(&capture.join("stdin.txt")),
        ),
        (
            "the request argv",
            common::read_file(&capture.join("argv.txt")),
        ),
        ("stdout", run.stdout.clone()),
        ("stderr", run.stderr.clone()),
    ];
    for file in ["transcript.txt", "ai-state.json", "proposed.json"] {
        everything.push((file, common::read_file(&dir.join(file))));
    }
    for (where_, text) in &everything {
        assert!(
            !text.contains(SENTINEL_KEY),
            "C14: a key from the environment appears in {where_}"
        );
        assert!(
            !text.contains(SENTINEL_TOKEN),
            "C14: a token from the environment appears in {where_}"
        );
    }
}
