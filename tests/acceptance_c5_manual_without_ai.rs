//! C5 — manual and headless use work with the AI off and no cloud account.
//! C2 — every check runs with no network.
mod acceptance_common;
use acceptance_common as common;
use common::Json;
use std::time::Duration;

#[test]
fn c5_eval_works_with_an_empty_environment_and_no_ai_command() {
    let dir = common::scratch("c5-eval");
    common::write(&dir.join("problem.json"), &common::simple_problem());
    let run = common::run_with(
        &common::catalyst_bin(),
        &["eval", "--problem", "problem.json"],
        &dir,
        true,
        &[],
        None,
        Duration::from_secs(common::PER_TEST_BOUND_SECS),
    );
    common::assert_ok(&run);
}

#[test]
fn c5_the_headless_guided_flow_works_with_an_empty_environment() {
    let dir = common::scratch("c5-tui");
    common::write(
        &dir.join("keys.txt"),
        &common::typed_flow(
            "func simple(x, y) = x * y + sin(x)",
            &[("x", 0.7, -3.0, 3.0, ""), ("y", 1.3, -2.0, 2.0, "")],
            "export/simple",
        ),
    );
    let run = common::run_with(
        &common::catalyst_bin(),
        &[
            "tui",
            "--headless",
            "--keys",
            "keys.txt",
            "--transcript",
            "transcript.txt",
        ],
        &dir,
        true,
        &[],
        None,
        Duration::from_secs(common::PER_TEST_BOUND_SECS),
    );
    assert!(!run.panicked(), "{}", run.summary());
    assert_eq!(run.status, Some(0), "{}", run.summary());
    assert!(
        dir.join("export/simple/function.go").is_file(),
        "the flow exports Go with no AI and no network:\n{}",
        run.summary()
    );
}

#[test]
fn c5_the_ai_feature_says_it_is_unavailable_and_manual_use_is_unaffected() {
    let dir = common::scratch("c5-ai-off");
    let empty = dir.join("empty-path");
    std::fs::create_dir_all(&empty).expect("empty PATH dir");
    let env = [
        ("PATH", empty.to_string_lossy().into_owned()),
        (
            "ANTHROPIC_API_KEY",
            "SENTINEL-C14-NEVER-PRINTED-9f3c".to_string(),
        ),
    ];
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let run = common::catalyst(&["ai", "status"], &dir, &pairs);
    assert!(!run.panicked(), "{}", run.summary());
    assert_eq!(
        run.status,
        Some(0),
        "ai status reports, it does not fail:\n{}",
        run.summary()
    );
    let json = run.json();
    assert_eq!(
        json.get("available").and_then(Json::as_bool),
        Some(false),
        "with no `claude` on PATH the AI is unavailable:\n{}",
        run.summary()
    );
    assert!(
        json.str_field("remedy").len() >= 8,
        "the status names the next step:\n{}",
        run.summary()
    );
    assert!(
        !run.stdout.contains("SENTINEL-C14") && !run.stderr.contains("SENTINEL-C14"),
        "C14: a credential in the environment is never echoed"
    );
    common::write(&dir.join("problem.json"), &common::simple_problem());
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &pairs);
    common::assert_ok(&run);
    assert!(!run.stdout.contains("SENTINEL-C14"));
}

#[test]
fn c2_the_check_lane_has_no_network() {
    use std::net::{SocketAddr, TcpStream};
    // C2 is a property of the check lane, which hands every check a scratch
    // directory as TMPDIR. Outside the lane (a developer's own `cargo test`)
    // there is nothing to measure, and that is said rather than failed.
    let tmp = std::env::var("TMPDIR").unwrap_or_default();
    if !tmp.contains("canaryio-check-scratch") {
        eprintln!("C2: not inside the CanaryIO check lane (TMPDIR={tmp:?}); the no-network property is measured only there");
        return;
    }
    let addr: SocketAddr = "1.1.1.1:80".parse().unwrap();
    let outcome = TcpStream::connect_timeout(&addr, Duration::from_secs(3));
    assert!(
        outcome.is_err(),
        "C2: a connection to a non-loopback address must fail inside a check"
    );
}
