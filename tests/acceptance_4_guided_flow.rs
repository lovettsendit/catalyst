//! Outcome 4 — manual, AI-off use through the guided flow Goal → Inputs →
//! Run → Results → Export (criteria 4.1–4.5). The headless driver feeds
//! scripted keystrokes; the rendered-WezTerm part (4.6) is a camera fact.
mod acceptance_common;
use acceptance_common as common;
use common::{Input, Json, Run};
use std::path::PathBuf;

const STATES: &[&str] = &["proposed", "running", "measured", "failed", "inconclusive"];

/// Drive the headless TUI with a key script; returns the scratch dir, the run
/// and the transcript text.
fn drive(name: &str, keys: &str, extra: &[&str]) -> (PathBuf, Run, String) {
    let dir = common::scratch(name);
    common::write(&dir.join("keys.txt"), keys);
    let mut args = vec![
        "tui",
        "--headless",
        "--keys",
        "keys.txt",
        "--transcript",
        "transcript.txt",
    ];
    args.extend_from_slice(extra);
    let run = common::catalyst(&args, &dir, &[]);
    let transcript = std::fs::read_to_string(dir.join("transcript.txt")).unwrap_or_default();
    (dir, run, transcript)
}

fn header_field(header: &str, key: &str) -> String {
    let Some(start) = header.find(&format!("{key}: ")) else {
        return String::new();
    };
    let rest = &header[start + key.len() + 2..];
    rest.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn steps(transcript: &str) -> Vec<String> {
    common::frames(transcript)
        .iter()
        .map(|(h, _)| header_field(h, "step"))
        .collect()
}

fn states(transcript: &str) -> Vec<String> {
    common::frames(transcript)
        .iter()
        .map(|(h, _)| header_field(h, "state"))
        .collect()
}

/// Type a problem up to and including the run, then quit.
fn typed_until_results(function: &str, inputs: &[Input]) -> String {
    let full = common::typed_flow(function, inputs, "unused");
    // Cut after the second `enter` following the inputs: Goal, Inputs, Run.
    let mut lines: Vec<&str> = full.lines().collect();
    // typed_flow ends with: enter (Inputs->Run), enter (Run->Results), enter (Results->Export), text:<out>, enter.
    lines.truncate(lines.len() - 3);
    lines.push("ctrl-c");
    common::keys(&lines)
}

fn assert_no_panic(run: &Run) {
    assert!(
        !run.panicked(),
        "the TUI must never panic:\n{}",
        run.summary()
    );
}

#[test]
fn oracle_4_1_the_guided_flow_completes_by_hand_with_ai_off() {
    let bound = common::Bound::start("4.1 guided flow");
    let (dir, run, transcript) = drive(
        "flow",
        &common::typed_flow(
            common::SPRING_FUNCTION,
            common::SPRING_INPUTS,
            "export/spring",
        ),
        &[],
    );
    assert_no_panic(&run);
    assert_eq!(run.status, Some(0), "{}", run.summary());
    let seen = steps(&transcript);
    assert!(
        !seen.is_empty(),
        "4.1: the transcript must carry frames (`--- frame N (step: …, state: …) ---`):\n{}",
        run.summary()
    );
    let mut cursor = 0;
    for expected in ["goal", "inputs", "run", "results", "export"] {
        let pos = seen[cursor..]
            .iter()
            .position(|s| s == expected)
            .unwrap_or_else(|| {
                panic!("4.1: step `{expected}` must follow the previous steps; frames: {seen:?}")
            });
        cursor += pos;
    }
    let (_, last_body) = common::frames(&transcript)
        .last()
        .cloned()
        .expect("a last frame");
    assert!(
        last_body.contains("exported:"),
        "4.1: the last frame reports the export:\n{last_body}"
    );
    for file in [
        "go.mod",
        "function.go",
        "main.go",
        "parameters.json",
        "fixtures.json",
        "validation-report.md",
    ] {
        assert!(
            dir.join("export/spring").join(file).is_file(),
            "4.1: the flow's export must contain {file}"
        );
    }
    for (_, body) in common::frames(&transcript) {
        assert!(
            body.contains("Goal > Inputs > Run > Results > Export"),
            "every frame shows the guided flow header:\n{body}"
        );
    }
    bound.check();
}

#[test]
fn oracle_4_2_headless_and_terminal_drives_produce_byte_identical_exports() {
    let bound = common::Bound::start("4.2 two drives");
    let dir = common::scratch("two-drives");
    common::write(
        &dir.join("keys-a.txt"),
        &common::typed_flow(common::SPRING_FUNCTION, common::SPRING_INPUTS, "a/spring"),
    );
    common::write(
        &dir.join("keys-b.txt"),
        &common::typed_flow(common::SPRING_FUNCTION, common::SPRING_INPUTS, "b/spring"),
    );
    let headless = common::catalyst(
        &[
            "tui",
            "--headless",
            "--keys",
            "keys-a.txt",
            "--transcript",
            "t.txt",
        ],
        &dir,
        &[],
    );
    assert_no_panic(&headless);
    assert_eq!(headless.status, Some(0), "{}", headless.summary());
    // The terminal renderer, driven by the same keys, stdout a pipe.
    let terminal = common::catalyst(
        &["tui", "--keys", "keys-b.txt"],
        &dir,
        &[("TERM", "xterm-256color")],
    );
    assert_no_panic(&terminal);
    assert_eq!(
        terminal.status,
        Some(0),
        "4.2: the terminal renderer must work with a scripted keyboard and a piped stdout:\n{}",
        terminal.summary()
    );
    assert!(
        terminal.stdout.contains("\u{1b}["),
        "4.2: the terminal drive renders with ANSI sequences, the headless one does not"
    );
    for file in [
        "go.mod",
        "function.go",
        "main.go",
        "parameters.json",
        "fixtures.json",
        "validation-report.md",
    ] {
        let a = std::fs::read(dir.join("a/spring").join(file))
            .unwrap_or_else(|e| panic!("headless export {file}: {e}"));
        let b = std::fs::read(dir.join("b/spring").join(file))
            .unwrap_or_else(|e| panic!("terminal export {file}: {e}"));
        assert!(
            a == b,
            "4.2: {file} differs between the headless and the terminal drive"
        );
    }
    bound.check();
}

#[test]
fn oracle_4_3_exactly_the_five_states_exist_and_render_distinguishably() {
    let dir = common::scratch("states");
    let run = common::catalyst(&["tui", "--states"], &dir, &[]);
    assert_no_panic(&run);
    let listed: Vec<String> = run
        .json()
        .as_arr()
        .unwrap_or_else(|| {
            panic!(
                "`catalyst tui --states` prints a JSON array:\n{}",
                run.summary()
            )
        })
        .iter()
        .map(|s| s.as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(listed, STATES, "4.3: exactly these states, in this order");

    let mut seen_all: Vec<String> = Vec::new();
    let mut record = |name: &str, keys: String, must_see: &[&str]| {
        let (_, run, transcript) = drive(name, &keys, &[]);
        assert_no_panic(&run);
        let frames = common::frames(&transcript);
        for (header, body) in &frames {
            let state = header_field(header, "state");
            assert!(
                STATES.contains(&state.as_str()),
                "4.3: frame header names a state outside the model: {header}"
            );
            assert!(
                body.contains(&format!("state: {state}")),
                "4.3: the frame body renders its state label `state: {state}`:\n{body}"
            );
            seen_all.push(state);
        }
        let states_here = states(&transcript);
        for want in must_see {
            assert!(
                states_here.iter().any(|s| s == want),
                "4.3: drive `{name}` must render state `{want}`; saw {states_here:?}\n{}",
                run.summary()
            );
        }
    };
    record(
        "state-measured",
        typed_until_results(common::SPRING_FUNCTION, common::SPRING_INPUTS),
        &["proposed", "running", "measured"],
    );
    record(
        "state-failed",
        typed_until_results("func f(x) = sqrt(x)", &[("x", -1.0, -2.0, 2.0, "")]),
        &["running", "failed"],
    );
    record(
        "state-inconclusive",
        typed_until_results("func f(x) = sqrt(x)", &[("x", 0.0, 0.0, 1.0, "")]),
        &["running", "inconclusive"],
    );
    for state in STATES {
        assert!(
            seen_all.iter().any(|s| s == state),
            "4.3: state `{state}` was never rendered"
        );
    }
}

#[test]
fn oracle_4_4_validation_messages_say_what_to_do() {
    let bad_goal = common::keys(&["text:func f(x) = +", "enter", "ctrl-c"]);
    let bad_number = common::keys(&[
        "text:func f(x) = x",
        "enter",
        "text:abc",
        "tab",
        "text:0",
        "tab",
        "text:2",
        "tab",
        "text:",
        "enter",
        "ctrl-c",
    ]);
    let bad_domain = common::keys(&[
        "text:func f(x) = x",
        "enter",
        "text:1",
        "tab",
        "text:3",
        "tab",
        "text:1",
        "tab",
        "text:",
        "enter",
        "ctrl-c",
    ]);
    let outside = common::keys(&[
        "text:func f(x) = x",
        "enter",
        "text:5",
        "tab",
        "text:0",
        "tab",
        "text:2",
        "tab",
        "text:",
        "enter",
        "ctrl-c",
    ]);
    let unknown = common::keys(&["text:func f(x) = x + q", "enter", "ctrl-c"]);
    for (name, keys) in [
        ("bad-goal", bad_goal),
        ("bad-number", bad_number),
        ("bad-domain", bad_domain),
        ("outside", outside),
        ("unknown", unknown),
    ] {
        let (_, run, transcript) = drive(&format!("validation-{name}"), &keys, &[]);
        assert_no_panic(&run);
        let frames = common::frames(&transcript);
        let with_problem = frames.iter().find(|(_, body)| body.contains("Problem:"));
        let (_, body) = with_problem.unwrap_or_else(|| {
            panic!(
                "4.4: drive `{name}` must render a `Problem:` line; frames: {}\n{}",
                transcript,
                run.summary()
            )
        });
        let next = body.lines().find_map(|l| l.trim().strip_prefix("Next step:")).unwrap_or_else(|| panic!("4.4: drive `{name}`: the frame with the problem must also say `Next step: …`:\n{body}"));
        assert!(
            next.trim().len() >= 8,
            "4.4: drive `{name}`: the next step must say what to do:\n{body}"
        );
        let bad = frames
            .iter()
            .any(|(h, _)| ["run", "results", "export"].contains(&header_field(h, "step").as_str()));
        assert!(
            !bad,
            "4.4: drive `{name}` must not advance past the invalid input"
        );
    }
}

#[test]
fn oracle_4_5_progress_is_visible_and_cancel_save_resume_keep_the_inputs() {
    // Progress.
    let (_, run, transcript) = drive(
        "progress",
        &typed_until_results(common::SPRING_FUNCTION, common::SPRING_INPUTS),
        &[],
    );
    assert_no_panic(&run);
    let running = common::frames(&transcript)
        .into_iter()
        .find(|(h, _)| header_field(h, "state") == "running");
    let (_, body) = running.expect("4.5: a frame in state running");
    assert!(
        body.contains("progress:"),
        "4.5: the running frame shows progress:\n{body}"
    );

    // Cancel from Results returns to Inputs with the same values.
    let mut lines: Vec<&str> = Vec::new();
    let full = common::typed_flow(common::SPRING_FUNCTION, common::SPRING_INPUTS, "unused");
    let all: Vec<&str> = full.lines().collect();
    lines.extend_from_slice(&all[..all.len() - 3]); // up to Run -> Results
    lines.push("esc");
    lines.push("ctrl-c");
    let (_, run, transcript) = drive("cancel", &common::keys(&lines), &[]);
    assert_no_panic(&run);
    let frames = common::frames(&transcript);
    let results_at = frames
        .iter()
        .position(|(h, _)| header_field(h, "step") == "results")
        .expect("4.5: reached Results");
    let after = frames.get(results_at + 1).expect("4.5: a frame after esc");
    assert_eq!(
        header_field(&after.0, "step"),
        "inputs",
        "4.5: esc from Results returns to Inputs"
    );
    for value in ["12", "1.5", "N/m", "N*s/m"] {
        assert!(
            after.1.contains(value),
            "4.5: the inputs are kept after cancel ({value} missing):\n{}",
            after.1
        );
    }

    // Save, then resume in a fresh process.
    let dir = common::scratch("save-resume");
    // The typed flow ends with the five events enter, enter, enter, text:<out>,
    // enter; everything before them is the goal and every input field.
    let mut save_lines: Vec<&str> = all[..all.len() - 5].to_vec();
    save_lines.push("ctrl-s");
    save_lines.push("ctrl-c");
    common::write(&dir.join("keys.txt"), &common::keys(&save_lines));
    let run = common::catalyst(
        &[
            "tui",
            "--headless",
            "--keys",
            "keys.txt",
            "--transcript",
            "t1.txt",
            "--save",
            "state.json",
        ],
        &dir,
        &[],
    );
    assert_no_panic(&run);
    let state = Json::parse(&common::read_file(&dir.join("state.json")))
        .expect("4.5: the saved state is JSON");
    assert_eq!(state.str_field("schema"), "catalyst.tui-state.v1");
    assert_eq!(state.str_field("step").to_ascii_lowercase(), "inputs");
    common::write(&dir.join("keys2.txt"), &common::keys(&["ctrl-c"]));
    let run = common::catalyst(
        &[
            "tui",
            "--headless",
            "--keys",
            "keys2.txt",
            "--transcript",
            "t2.txt",
            "--resume",
            "state.json",
        ],
        &dir,
        &[],
    );
    assert_no_panic(&run);
    let transcript = common::read_file(&dir.join("t2.txt"));
    let (header, body) = common::frames(&transcript)
        .into_iter()
        .next()
        .expect("4.5: a first frame after resume");
    assert_eq!(
        header_field(&header, "step"),
        "inputs",
        "4.5: resume returns to the same step"
    );
    for value in ["12", "1.5", "N/m", "N*s/m"] {
        assert!(
            body.contains(value),
            "4.5: resume restores the same inputs ({value} missing):\n{body}"
        );
    }
    // The resumed session can still finish: run and export.
    let mut finish: Vec<&str> = vec!["enter", "enter", "enter", "text:export/resumed", "enter"];
    finish.push("ctrl-c");
    common::write(&dir.join("keys3.txt"), &common::keys(&finish));
    let run = common::catalyst(
        &[
            "tui",
            "--headless",
            "--keys",
            "keys3.txt",
            "--transcript",
            "t3.txt",
            "--resume",
            "state.json",
        ],
        &dir,
        &[],
    );
    assert_no_panic(&run);
    assert!(
        dir.join("export/resumed/function.go").is_file(),
        "4.5: the resumed flow completes to an export:\n{}",
        run.summary()
    );
}
