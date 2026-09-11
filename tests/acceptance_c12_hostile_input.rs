//! C12 — hostile input is refused with a validation error that names a next
//! step, never a panic: over-long and over-nested expressions, non-finite
//! numbers, malformed or empty fixtures, unknown variables.
mod acceptance_common;
use acceptance_common as common;
use common::Json;

fn eval_text(name: &str, problem_text: &str) -> common::Run {
    let dir = common::scratch(name);
    common::write(&dir.join("problem.json"), problem_text);
    common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[])
}

#[test]
fn c12_an_expression_over_the_declared_length_is_refused() {
    let body = format!("x{}", " + x".repeat(20_000)); // about 80 KiB, over the 64 KiB limit
    let function = format!("func long(x) = {body}");
    let problem = common::problem_json("long", "too long", &function, &[("x", 1.0, 0.0, 2.0, "")]);
    let run = eval_text("c12-length", &problem);
    common::assert_refusal(&run, "catalyst.function_too_long");
}

#[test]
fn c12_an_expression_over_the_nesting_depth_is_refused() {
    let function = format!("func deep(x) = {}x{}", "(".repeat(65), ")".repeat(65));
    let problem = common::problem_json("deep", "too deep", &function, &[("x", 1.0, 0.0, 2.0, "")]);
    let run = eval_text("c12-depth", &problem);
    common::assert_refusal(&run, "catalyst.");
    let very_deep = format!(
        "func deep(x) = {}x{}",
        "(".repeat(20_000),
        ")".repeat(20_000)
    );
    let problem = common::problem_json(
        "deep",
        "far too deep",
        &very_deep,
        &[("x", 1.0, 0.0, 2.0, "")],
    );
    let run = eval_text("c12-depth-huge", &problem);
    common::assert_refusal(&run, "catalyst.");
}

#[test]
fn c12_non_finite_numbers_are_refused() {
    for (label, text) in [
        (
            "overflowing literal",
            r#"{"schema":"catalyst.problem.v1","name":"nf","goal":"g","function":"func nf(x) = x","inputs":{"x":1e999},"domains":{"x":{"min":0,"max":2,"unit":""}}}"#,
        ),
        (
            "NaN literal",
            r#"{"schema":"catalyst.problem.v1","name":"nf","goal":"g","function":"func nf(x) = x","inputs":{"x":NaN},"domains":{"x":{"min":0,"max":2,"unit":""}}}"#,
        ),
        (
            "Infinity literal",
            r#"{"schema":"catalyst.problem.v1","name":"nf","goal":"g","function":"func nf(x) = x","inputs":{"x":1},"domains":{"x":{"min":-Infinity,"max":2,"unit":""}}}"#,
        ),
        (
            "overflowing domain",
            r#"{"schema":"catalyst.problem.v1","name":"nf","goal":"g","function":"func nf(x) = x","inputs":{"x":1},"domains":{"x":{"min":0,"max":1e999,"unit":""}}}"#,
        ),
    ] {
        let run = eval_text("c12-nonfinite", text);
        let json = common::assert_refusal(&run, "catalyst.");
        assert!(
            !run.stdout.contains("1e999"),
            "{label}: the refusal must not echo the hostile literal back unchanged"
        );
        let _ = json;
    }
}

#[test]
fn c12_malformed_and_empty_fixtures_are_refused() {
    for (label, text) in [
        ("empty file", ""),
        ("whitespace only", "  \n"),
        (
            "truncated object",
            r#"{"schema":"catalyst.problem.v1","name":"#,
        ),
        ("not an object", r#"["catalyst.problem.v1"]"#),
        (
            "wrong types",
            r#"{"schema":"catalyst.problem.v1","name":5,"goal":[],"function":{},"inputs":"x","domains":1}"#,
        ),
        (
            "wrong schema",
            r#"{"schema":"catalyst.something.v9","name":"a","goal":"g","function":"func a(x) = x","inputs":{"x":1},"domains":{"x":{"min":0,"max":2,"unit":""}}}"#,
        ),
        ("binary garbage", "\u{0}\u{1}\u{2}{{{{"),
        ("deeply nested arrays", &"[".repeat(200_000)),
    ] {
        let run = eval_text("c12-malformed", text);
        common::assert_refusal(&run, "catalyst.");
        assert!(
            run.status == Some(2),
            "{label}: exit code 2 expected:\n{}",
            run.summary()
        );
    }
    // The same fixtures through the export command write nothing.
    let dir = common::scratch("c12-export-malformed");
    common::write(
        &dir.join("problem.json"),
        r#"{"schema":"catalyst.problem.v1","name":"#,
    );
    let run = common::catalyst(
        &["export", "go", "--problem", "problem.json", "--out", "out"],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.");
    assert!(
        !dir.join("out").exists(),
        "a refused export must create nothing"
    );
}

#[test]
fn c12_unknown_variables_are_refused_with_a_next_step() {
    let dir = common::scratch("c12-unknown");
    // A name used in the body that is not a parameter.
    let problem = common::problem_json("u", "g", "func u(x) = x + q", &[("x", 1.0, 0.0, 2.0, "")]);
    common::write(&dir.join("problem.json"), &problem);
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[]);
    let json = common::assert_refusal(&run, "catalyst.unknown_name");
    assert!(
        json.str_field("detail").contains('q'),
        "the refusal names the unknown variable:\n{}",
        run.summary()
    );
    // An input that is not a parameter.
    let text = r#"{"schema":"catalyst.problem.v1","name":"u","goal":"g","function":"func u(x) = x","inputs":{"x":1,"z":2},"domains":{"x":{"min":0,"max":2,"unit":""},"z":{"min":0,"max":3,"unit":""}}}"#;
    common::write(&dir.join("problem.json"), text);
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[]);
    common::assert_refusal(&run, "catalyst.unknown_name");
    // A parameter with no input or no domain.
    let text = r#"{"schema":"catalyst.problem.v1","name":"u","goal":"g","function":"func u(x, y) = x * y","inputs":{"x":1},"domains":{"x":{"min":0,"max":2,"unit":""}}}"#;
    common::write(&dir.join("problem.json"), text);
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[]);
    let json = common::assert_refusal(&run, "catalyst.problem_incomplete");
    assert!(
        json.str_field("detail").contains('y'),
        "the refusal names the missing parameter:\n{}",
        run.summary()
    );
    // An input outside its own domain.
    let text = r#"{"schema":"catalyst.problem.v1","name":"u","goal":"g","function":"func u(x) = x","inputs":{"x":5},"domains":{"x":{"min":0,"max":2,"unit":""}}}"#;
    common::write(&dir.join("problem.json"), text);
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[]);
    common::assert_refusal(&run, "catalyst.input_out_of_domain");
    // min not below max.
    let text = r#"{"schema":"catalyst.problem.v1","name":"u","goal":"g","function":"func u(x) = x","inputs":{"x":1},"domains":{"x":{"min":2,"max":2,"unit":""}}}"#;
    common::write(&dir.join("problem.json"), text);
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[]);
    common::assert_refusal(&run, "catalyst.");
}

#[test]
fn c12_a_valid_problem_evaluates_so_the_refusals_above_are_not_a_dead_command() {
    let dir = common::scratch("c12-valid");
    common::write(&dir.join("problem.json"), &common::simple_problem());
    let run = common::catalyst(&["eval", "--problem", "problem.json"], &dir, &[]);
    let json = common::assert_ok(&run);
    let value = json.num_field("value");
    assert!(
        common::within(value, 0.7 * 1.3 + 0.7f64.sin(), 1e-12, 1e-12),
        "value {value}"
    );
    let gx = json
        .path("gradient.x")
        .and_then(Json::as_f64)
        .expect("gradient.x");
    assert!(
        common::within(gx, 1.3 + 0.7f64.cos(), 1e-12, 1e-12),
        "df/dx {gx}"
    );
    assert_eq!(json.get("finite").and_then(Json::as_bool), Some(true));
}
