//! Amendment 9 — the Catalyst Gradient Compiler (criteria 12.1–12.13,
//! `docs/interface.md` §12). Every module differentiated here is real
//! compiler output: `tests/fixtures/llvm/*.ll` were written by `rustc
//! --emit=llvm-ir -O` from the Rust beside them, so the front end is
//! exercised on what a compiler emits, not on IR written to be easy. Every
//! number CGC reports is checked against a closed form worked out by hand,
//! which is independent of the engine, of the verifier and of the fixtures.
mod acceptance_common;
use acceptance_common as common;
use common::Json;
use std::path::{Path, PathBuf};

const FILES: &[&str] = &[
    "derivative.json",
    "module.ll",
    "primal.cir",
    "derivative.cir",
    "fixtures.json",
    "validation-report.md",
];
const REL: f64 = 1e-9;
const ABS: f64 = 1e-12;
const SENTENCE: &str =
    "The derivative was produced by the Catalyst Gradient Compiler and checked by \
                        Catalyst's own verifier; neither one declares the other correct.";

/// A scratch directory holding the named fixture as `module.ll`.
fn with_module(name: &str, fixture: &str) -> PathBuf {
    let dir = common::scratch(name);
    common::write(&dir.join("module.ll"), &common::fixture(fixture));
    dir
}

fn differentiate(dir: &Path, extra: &[&str]) -> common::Run {
    let mut args = vec!["differentiate", "llvm", "--module", "module.ll"];
    args.extend_from_slice(extra);
    common::catalyst(&args, dir, &[])
}

fn number(json: &Json, path: &str) -> f64 {
    json.path(path)
        .and_then(Json::as_f64)
        .unwrap_or_else(|| panic!("12: `{path}` is not a number in {}", json.render()))
}

fn close(got: f64, want: f64, what: &str) {
    assert!(
        common::within(got, want, REL, ABS),
        "12: {what}: got {got}, closed form {want}"
    );
}

// heat(x, y) = x * exp(y > 1 ? 0.8 y : y)
fn heat(x: f64, y: f64) -> (f64, f64, f64) {
    let (adj, slope) = if y > 1.0 { (0.8 * y, 0.8) } else { (y, 1.0) };
    let v = x * adj.exp();
    (v, adj.exp(), x * adj.exp() * slope)
}

// cost(x, n) = sum_{i<n} sin(x i)
fn cost(x: f64, n: i64) -> (f64, f64) {
    let mut v = 0.0;
    let mut d = 0.0;
    for i in 0..n {
        let i = i as f64;
        v += (x * i).sin();
        d += i * (x * i).cos();
    }
    (v, d)
}

fn assert_assurance(json: &Json, kind: &str, backend: &str, portable: bool) {
    assert_eq!(
        json.path("assurance.computation_kind")
            .and_then(Json::as_str),
        Some(kind)
    );
    assert_eq!(
        json.path("assurance.differentiation_backend")
            .and_then(Json::as_str),
        Some(backend)
    );
    assert_eq!(
        json.path("assurance.validated").and_then(Json::as_bool),
        Some(true)
    );
    assert_eq!(
        json.path("assurance.portable_export")
            .and_then(Json::as_bool),
        Some(portable),
        "12: portable_export for {kind}"
    );
}

fn assert_validation(json: &Json, key: &str, points: f64) {
    assert_eq!(
        number(json, &format!("{key}.points")),
        points,
        "12: validation points"
    );
    assert_eq!(
        number(json, &format!("{key}.passed")),
        points,
        "12: every point passed"
    );
    assert_eq!(number(json, &format!("{key}.relative_tolerance")), 1e-6);
    assert_eq!(number(json, &format!("{key}.absolute_tolerance")), 1e-9);
}

#[test]
fn sha256_helper_matches_the_published_vectors() {
    assert_eq!(
        common::sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        common::sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn oracle_12_1_describe_names_two_backends_and_runs_nothing() {
    let dir = common::scratch("describe");
    let before = common::listing(&dir);
    let run = common::run_with(
        &common::catalyst_bin(),
        &["differentiate", "--describe"],
        &dir,
        true,
        &[],
        None,
        std::time::Duration::from_secs(common::PER_TEST_BOUND_SECS),
    );
    assert_eq!(run.status, Some(0), "12.1:\n{}", run.summary());
    let json = run.json();
    assert_eq!(json.str_field("schema"), "catalyst.differentiation.v1");
    assert_eq!(json.str_field("artifact"), "catalyst.derivative.v1");
    let backends = json
        .get("backends")
        .and_then(Json::as_arr)
        .expect("12.1: backends");
    let names: Vec<&str> = backends.iter().map(|b| b.str_field("name")).collect();
    assert_eq!(
        names,
        ["native", "cgc"],
        "12.1: the two backends, in this order"
    );
    assert_eq!(backends[0].str_field("input"), "catalyst-ir");
    assert_eq!(backends[1].str_field("input"), "llvm-ir");
    let modes: Vec<&str> = backends[1]
        .get("modes")
        .and_then(Json::as_arr)
        .expect("12.1: cgc modes")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    assert_eq!(modes, ["forward", "reverse"]);
    assert_eq!(number(&json, "validation.relative_tolerance"), 1e-6);
    assert_eq!(number(&json, "validation.absolute_tolerance"), 1e-9);
    assert_eq!(
        common::listing(&dir),
        before,
        "12.1: --describe wrote nothing"
    );
}

#[test]
fn oracle_12_2_a_branching_function_from_rustc_on_both_sides_of_its_branch() {
    for (x, y) in [(1.5, 2.0), (1.5, 0.5), (-0.75, 1.25)] {
        let dir = with_module("heat", "llvm/programs.ll");
        let at = format!("{x},{y}");
        let run = differentiate(
            &dir,
            &[
                "--function",
                "heat",
                "--inputs",
                "x,y",
                "--at",
                &at,
                "--out",
                "out",
            ],
        );
        let json = common::assert_ok(&run);
        assert_eq!(json.str_field("backend"), "cgc");
        assert_eq!(
            json.str_field("mode"),
            "reverse",
            "12.2: a select is not a branch, so reverse"
        );
        assert_eq!(json.str_field("function"), "heat");
        assert_eq!(json.str_field("out"), "out");
        assert_eq!(number(&json, "at.x"), x);
        assert_eq!(number(&json, "at.y"), y);
        let (v, dx, dy) = heat(x, y);
        close(number(&json, "value"), v, "heat value");
        close(number(&json, "gradient.x"), dx, "d heat / d x");
        close(number(&json, "gradient.y"), dy, "d heat / d y");
        assert_eq!(json.get("finite").and_then(Json::as_bool), Some(true));
        assert_validation(&json, "validation", 5.0);
        assert_assurance(&json, "llvm", "cgc", false);
        let files: Vec<&str> = json
            .get("files")
            .and_then(Json::as_arr)
            .expect("12.2: files")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        assert_eq!(files, FILES);
        assert_eq!(common::listing(&dir.join("out")), {
            let mut f: Vec<String> = FILES.iter().map(|s| s.to_string()).collect();
            f.sort();
            f
        });
    }
}

#[test]
fn oracle_12_3_a_loop_from_rustc_is_differentiated_forward() {
    // n = 5 runs the four-way unrolled body once and the epilogue once; n = 6
    // takes the epilogue twice; n = 3 never enters the unrolled body; n = 0
    // never enters the loop. Four different paths through the same IR.
    for n in [5i64, 6, 3, 0] {
        let dir = with_module("cost", "llvm/programs.ll");
        let x = 0.7;
        let at = format!("{x},{n}");
        let run = differentiate(
            &dir,
            &[
                "--function",
                "cost",
                "--inputs",
                "x",
                "--at",
                &at,
                "--out",
                "out",
            ],
        );
        let json = common::assert_ok(&run);
        assert_eq!(
            json.str_field("mode"),
            "forward",
            "12.3: a loop is differentiated forward"
        );
        let (v, d) = cost(x, n);
        close(number(&json, "value"), v, &format!("cost value, n={n}"));
        close(
            number(&json, "gradient.x"),
            d,
            &format!("d cost / d x, n={n}"),
        );
        assert_eq!(number(&json, "at.n"), n as f64);
        assert!(
            json.path("gradient.n").is_none(),
            "12.3: n was not an input"
        );
        assert_validation(&json, "validation", 3.0);
    }
}

#[test]
fn oracle_12_4_the_artifact_carries_its_provenance_chain_and_is_deterministic() {
    let dir = with_module("artifact", "llvm/programs.ll");
    common::write(
        &dir.join("programs.rs"),
        &common::fixture("llvm/programs.rs"),
    );
    let args = [
        "--function",
        "heat",
        "--inputs",
        "x,y",
        "--at",
        "1.5,2",
        "--out",
        "out",
        "--source",
        "programs.rs",
    ];
    let json = common::assert_ok(&differentiate(&dir, &args));
    let out = dir.join("out");
    let module = common::read_file(&out.join("module.ll"));
    assert_eq!(
        module,
        common::fixture("llvm/programs.ll"),
        "12.4: module.ll is the input, byte for byte"
    );
    let doc = Json::parse(&common::read_file(&out.join("derivative.json")))
        .expect("12.4: derivative.json parses");
    assert_eq!(doc.str_field("schema"), "catalyst.derivative.v1");
    assert_eq!(doc.str_field("backend"), "cgc");
    assert_eq!(doc.str_field("mode"), "reverse");
    assert_eq!(doc.str_field("function"), "heat");
    let strings = |key: &str| -> Vec<String> {
        doc.get(key)
            .and_then(Json::as_arr)
            .unwrap_or_else(|| panic!("12.4: `{key}` is an array"))
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(strings("inputs"), ["x", "y"]);
    assert_eq!(strings("outputs"), ["heat"]);
    assert_eq!(strings("rules_applied"), Vec::<String>::new());
    let params = doc
        .get("parameters")
        .and_then(Json::as_arr)
        .expect("12.4: parameters");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].str_field("name"), "x");
    assert_eq!(params[0].str_field("type"), "double");
    // The chain: source -> compiler IR -> primal IR -> derivative -> validation.
    let digest = |key: &str| doc.str_field(key).to_owned();
    let is_sha = |d: &str| {
        d.len() == 64
            && d.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    };
    for key in [
        "source_digest",
        "compiler_ir_digest",
        "primal_ir_digest",
        "derivative_digest",
        "validation_digest",
    ] {
        assert!(
            is_sha(&digest(key)),
            "12.4: `{key}` is a lowercase SHA-256, got {:?}",
            digest(key)
        );
    }
    assert_eq!(
        digest("source_digest"),
        common::sha256_hex(common::fixture("llvm/programs.rs").as_bytes())
    );
    assert_eq!(doc.str_field("source_name"), "programs.rs");
    assert_eq!(
        digest("compiler_ir_digest"),
        common::sha256_hex(module.as_bytes())
    );
    let primal = common::read_file(&out.join("primal.cir"));
    let derivative = common::read_file(&out.join("derivative.cir"));
    let fixtures = common::read_file(&out.join("fixtures.json"));
    assert_eq!(
        digest("primal_ir_digest"),
        common::sha256_hex(primal.as_bytes())
    );
    assert_eq!(
        digest("derivative_digest"),
        common::sha256_hex(derivative.as_bytes())
    );
    assert_eq!(
        digest("validation_digest"),
        common::sha256_hex(fixtures.as_bytes())
    );
    assert!(
        primal.contains("heat"),
        "12.4: primal.cir names the function"
    );
    assert!(
        !derivative.trim().is_empty() && derivative != primal,
        "12.4: derivative.cir is a different program"
    );
    assert!(
        doc.path("provenance.compiler")
            .and_then(Json::as_str)
            .is_some_and(|c| c.starts_with("rustc version")),
        "12.4: the compiler is read from !llvm.ident"
    );
    assert_eq!(
        doc.path("provenance.target_triple").and_then(Json::as_str),
        Some("x86_64-unknown-linux-gnu")
    );
    assert_eq!(
        doc.path("provenance.cgc_version").and_then(Json::as_str),
        Some(common::crate_version().as_str())
    );
    assert_eq!(
        doc.path("provenance.mode").and_then(Json::as_str),
        Some("reverse")
    );
    assert_eq!(
        doc.path("provenance.output").and_then(Json::as_str),
        Some("heat")
    );
    assert_assurance(&doc, "llvm", "cgc", false);
    assert_validation(&doc, "gradient_validation", 5.0);
    // The fixtures are the verifier's evidence: every case carries both numbers.
    let fx = Json::parse(&fixtures).expect("12.4: fixtures.json parses");
    assert_eq!(fx.str_field("schema"), "catalyst.derivative-fixtures.v1");
    assert_eq!(number(&fx, "tolerance.relative"), 1e-6);
    let cases = fx.get("cases").and_then(Json::as_arr).expect("12.4: cases");
    assert_eq!(cases.len(), 5);
    for case in cases {
        assert!(case.str_field("case").len() >= 2);
        assert_eq!(case.get("agrees").and_then(Json::as_bool), Some(true));
        let x = number(case, "at.x");
        let y = number(case, "at.y");
        let (v, dx, dy) = heat(x, y);
        close(number(case, "value"), v, "fixture value");
        close(number(case, "gradient.x"), dx, "fixture d/dx");
        close(number(case, "gradient.y"), dy, "fixture d/dy");
        assert!(
            common::within(number(case, "estimate.x"), dx, 1e-6, 1e-9),
            "12.4: the estimate is a real estimate"
        );
    }
    let report = common::read_file(&out.join("validation-report.md"));
    assert!(
        report.contains("## Remaining uncertainty"),
        "12.4: the report says what is not known"
    );
    assert!(
        report.contains(SENTENCE),
        "12.4: the report carries the sentence verbatim"
    );
    assert!(report.contains("reverse") && report.contains("heat"));
    // Nothing personal, nothing timed, in any file.
    let home = std::env::var("HOME").unwrap_or_default();
    for name in FILES {
        let text = common::read_file(&out.join(name));
        // Assembled at run time so C4's scan of this tree does not find its
        // own patterns spelled out here, the lesson generation 0 taught.
        let linux_home = ["/ho", "me/"].concat();
        let mac_home = ["/Us", "ers/"].concat();
        assert!(
            !text.contains(&linux_home) && !text.contains(&mac_home),
            "12.4: {name} carries an absolute path"
        );
        assert!(
            home.len() < 4 || !text.contains(&home),
            "12.4: {name} carries the home directory"
        );
        assert!(
            !text.contains(&format!("{}", dir.display())),
            "12.4: {name} carries the scratch path"
        );
    }
    assert_eq!(json.str_field("out"), "out");
    // Determinism: the same request again is the same bytes.
    let again = with_module("artifact-again", "llvm/programs.ll");
    common::write(
        &again.join("programs.rs"),
        &common::fixture("llvm/programs.rs"),
    );
    common::assert_ok(&differentiate(&again, &args));
    for name in FILES {
        assert_eq!(
            common::read_file(&out.join(name)),
            common::read_file(&again.join("out").join(name)),
            "12.4: {name} differs between two runs of one request"
        );
    }
}

const RULES: &str = r#"{"schema":"catalyst.derivative-rules.v1","rules":[{"function":"special_lookup","derivative":"special_lookup_gradient"}]}"#;

#[test]
fn oracle_12_5_a_registered_rule_is_used_and_a_wrong_one_is_caught_by_the_verifier() {
    // table_cost(x, y) = (x y)^3 + y^2, through a call rustc did not inline.
    let (x, y): (f64, f64) = (1.3, 0.9);
    let dx = 3.0 * (x * y).powi(2) * y;
    let dy = 3.0 * (x * y).powi(2) * x + 2.0 * y;
    let v = (x * y).powi(3) + y * y;
    let args = [
        "--function",
        "table_cost",
        "--inputs",
        "x,y",
        "--at",
        "1.3,0.9",
        "--out",
        "out",
    ];
    // Without a rule: the call is differentiated through.
    let plain = with_module("rules-none", "llvm/rules-right.ll");
    let json = common::assert_ok(&differentiate(&plain, &args));
    close(number(&json, "value"), v, "table_cost value");
    close(
        number(&json, "gradient.x"),
        dx,
        "d table_cost / d x, inlined",
    );
    close(
        number(&json, "gradient.y"),
        dy,
        "d table_cost / d y, inlined",
    );
    // With a correct rule: used, recorded, and still checked.
    let right = with_module("rules-right", "llvm/rules-right.ll");
    common::write(&right.join("rules.json"), RULES);
    let mut with_rules = args.to_vec();
    with_rules.extend_from_slice(&["--rules", "rules.json"]);
    let json = common::assert_ok(&differentiate(&right, &with_rules));
    close(
        number(&json, "gradient.x"),
        dx,
        "d table_cost / d x, by rule",
    );
    close(
        number(&json, "gradient.y"),
        dy,
        "d table_cost / d y, by rule",
    );
    let doc = Json::parse(&common::read_file(
        &right.join("out").join("derivative.json"),
    ))
    .unwrap();
    let applied: Vec<&str> = doc
        .get("rules_applied")
        .and_then(Json::as_arr)
        .expect("12.5: rules_applied")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    assert_eq!(
        applied,
        ["special_lookup"],
        "12.5: the rule that applied is recorded"
    );
    assert!(
        right.join("out").join("rules.json").is_file(),
        "12.5: the rules travel with the artifact"
    );
    // With a wrong rule (gradient 2x for x^3): the verifier, not CGC, decides.
    let wrong = with_module("rules-wrong", "llvm/rules-wrong.ll");
    common::write(&wrong.join("rules.json"), RULES);
    let run = differentiate(&wrong, &with_rules);
    let refusal = common::assert_refusal(&run, "catalyst.derivative_disagrees");
    let detail = refusal.str_field("detail");
    assert!(
        detail.contains('x'),
        "12.5: the refusal names the input:\n{detail}"
    );
    assert!(
        !wrong.join("out").exists() || common::listing(&wrong.join("out")).is_empty(),
        "12.5: a refused derivative writes nothing"
    );
    // A rule naming a function the module does not define is refused by name.
    let missing = with_module("rules-missing", "llvm/rules-right.ll");
    common::write(
        &missing.join("rules.json"),
        r#"{"schema":"catalyst.derivative-rules.v1","rules":[{"function":"special_lookup","derivative":"nowhere"}]}"#,
    );
    let refusal = common::assert_refusal(
        &differentiate(&missing, &with_rules),
        "catalyst.rule_invalid",
    );
    assert!(refusal.str_field("detail").contains("nowhere"));
}

#[test]
fn oracle_12_6_run_reexecutes_an_artifact_and_refuses_a_tampered_one() {
    let dir = with_module("run", "llvm/programs.ll");
    let made = common::assert_ok(&differentiate(
        &dir,
        &[
            "--function",
            "heat",
            "--inputs",
            "x,y",
            "--at",
            "1.5,2",
            "--out",
            "out",
        ],
    ));
    let run = common::catalyst(
        &["differentiate", "run", "--artifact", "out", "--at", "1.5,2"],
        &dir,
        &[],
    );
    let json = common::assert_ok(&run);
    assert_eq!(
        json.get("artifact_verified").and_then(Json::as_bool),
        Some(true)
    );
    assert_eq!(json.str_field("backend"), "cgc");
    assert_eq!(json.str_field("mode"), "reverse");
    assert_eq!(json.str_field("function"), "heat");
    assert_eq!(
        number(&json, "value"),
        number(&made, "value"),
        "12.6: the same number as when the artifact was made"
    );
    assert_eq!(number(&json, "gradient.x"), number(&made, "gradient.x"));
    assert_eq!(number(&json, "gradient.y"), number(&made, "gradient.y"));
    // Another point, against the closed form.
    let run = common::catalyst(
        &[
            "differentiate",
            "run",
            "--artifact",
            "out",
            "--at",
            "0.4,0.3",
        ],
        &dir,
        &[],
    );
    let json = common::assert_ok(&run);
    let (v, dx, dy) = heat(0.4, 0.3);
    close(number(&json, "value"), v, "run value");
    close(number(&json, "gradient.x"), dx, "run d/dx");
    close(number(&json, "gradient.y"), dy, "run d/dy");
    // Tamper with the module: one comment line changes the digest.
    let module_path = dir.join("out").join("module.ll");
    let original = common::read_file(&module_path);
    common::write(
        &module_path,
        &format!("{original}; edited after the fact\n"),
    );
    let run = common::catalyst(
        &["differentiate", "run", "--artifact", "out", "--at", "1.5,2"],
        &dir,
        &[],
    );
    let refusal = common::assert_refusal(&run, "catalyst.artifact_tampered");
    assert!(
        refusal.str_field("detail").contains("compiler_ir_digest"),
        "12.6: names the digest that failed"
    );
    common::write(&module_path, &original);
    // Tamper with the recorded derivative digest.
    let doc_path = dir.join("out").join("derivative.json");
    let doc = common::read_file(&doc_path);
    let key = "\"derivative_digest\":\"";
    let at = doc.find(key).expect("12.6: derivative_digest present") + key.len();
    let mut edited = doc.clone();
    let flipped = if &doc[at..at + 1] == "0" { "1" } else { "0" };
    edited.replace_range(at..at + 1, flipped);
    common::write(&doc_path, &edited);
    let run = common::catalyst(
        &["differentiate", "run", "--artifact", "out", "--at", "1.5,2"],
        &dir,
        &[],
    );
    let refusal = common::assert_refusal(&run, "catalyst.artifact_tampered");
    assert!(refusal.str_field("detail").contains("derivative_digest"));
    common::write(&doc_path, &doc);
    // Edit the listing a person reads, leaving every digest intact: the
    // derivative that runs must be the derivative that was shown.
    let listing_path = dir.join("out").join("derivative.cir");
    let listing = common::read_file(&listing_path);
    common::write(
        &listing_path,
        &format!("{listing}; edited after the fact\n"),
    );
    let run = common::catalyst(
        &["differentiate", "run", "--artifact", "out", "--at", "1.5,2"],
        &dir,
        &[],
    );
    let refusal = common::assert_refusal(&run, "catalyst.artifact_tampered");
    assert!(
        refusal.str_field("detail").contains("derivative.cir"),
        "12.6: an edited listing is named"
    );
}

#[test]
fn oracle_12_7_every_refusal_names_the_place_and_nothing_panics() {
    let heat = [
        "--function",
        "heat",
        "--inputs",
        "x,y",
        "--at",
        "1.5,2",
        "--out",
        "out",
    ];
    // Memory is outside the phase-1 subset, and the refusal says where.
    let dir = with_module("memory", "llvm/memory.ll");
    let run = differentiate(
        &dir,
        &[
            "--function",
            "dot",
            "--inputs",
            "n",
            "--at",
            "0,0,3",
            "--out",
            "out",
        ],
    );
    let refusal = common::assert_refusal(&run, "catalyst.llvm_unsupported");
    let detail = refusal.str_field("detail").to_ascii_lowercase();
    assert!(
        ["ptr", "load", "getelementptr", "store"]
            .iter()
            .any(|w| detail.contains(w)),
        "12.7: names what fell outside the subset:\n{detail}"
    );
    assert!(
        detail.contains("line") && detail.chars().any(|c| c.is_ascii_digit()),
        "12.7: names the line:\n{detail}"
    );
    // Text that is not LLVM IR.
    let dir = common::scratch("syntax");
    common::write(
        &dir.join("module.ll"),
        "define double @f(double %x) {\nstart:\n  %y = fadd double %x,\n  ret double %y\n}\n",
    );
    let refusal = common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "f",
                "--inputs",
                "x",
                "--at",
                "1",
                "--out",
                "out",
            ],
        ),
        "catalyst.llvm_syntax",
    );
    assert!(
        refusal
            .str_field("detail")
            .chars()
            .any(|c| c.is_ascii_digit()),
        "12.7: a syntax refusal carries a line"
    );
    // The function is not there, and the ones that are get named.
    let dir = with_module("missing", "llvm/programs.ll");
    let refusal = common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "drag",
                "--inputs",
                "x",
                "--at",
                "1",
                "--out",
                "out",
            ],
        ),
        "catalyst.llvm_function_not_found",
    );
    let detail = refusal.str_field("detail");
    assert!(
        detail.contains("heat") && detail.contains("cost"),
        "12.7: lists the defines:\n{detail}"
    );
    // An integer parameter cannot be an input; a name that is not a parameter is unknown.
    let dir = with_module("inputs", "llvm/programs.ll");
    common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "cost",
                "--inputs",
                "n",
                "--at",
                "0.7,5",
                "--out",
                "out",
            ],
        ),
        "catalyst.llvm_input_not_real",
    );
    common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "cost",
                "--inputs",
                "q",
                "--at",
                "0.7,5",
                "--out",
                "out",
            ],
        ),
        "catalyst.unknown_name",
    );
    common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "heat",
                "--inputs",
                "x",
                "--at",
                "1",
                "--out",
                "out",
            ],
        ),
        "catalyst.wrong_arity",
    );
    common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "cost",
                "--inputs",
                "x",
                "--at",
                "0.7,2.5",
                "--out",
                "out",
            ],
        ),
        "catalyst.llvm_argument_invalid",
    );
    common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "heat",
                "--inputs",
                "x,y",
                "--at",
                "1e999,2",
                "--out",
                "out",
            ],
        ),
        "catalyst.llvm_argument_invalid",
    );
    // A forced mode the engine refuses is refused with the engine's own code.
    let run = differentiate(
        &dir,
        &[
            "--function",
            "cost",
            "--inputs",
            "x",
            "--at",
            "0.7,5",
            "--out",
            "out",
            "--mode",
            "reverse",
        ],
    );
    let refusal = common::assert_refusal(&run, "catalyst.");
    let code = refusal.str_field("code");
    assert!(
        code == "catalyst.cyclic_control_flow" || code == "catalyst.branching_control_flow",
        "12.7: a forced reverse mode on a loop is the engine's refusal, not a silent swap: {code}"
    );
    assert!(!dir.join("out").exists(), "12.7: refusals write nothing");
    // The path rule, as everywhere.
    common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "heat",
                "--inputs",
                "x,y",
                "--at",
                "1.5,2",
                "--out",
                "/etc/nope",
            ],
        ),
        "catalyst.path_refused",
    );
    let run = common::catalyst(
        &[
            "differentiate",
            "llvm",
            "--module",
            "../module.ll",
            "--function",
            "heat",
            "--inputs",
            "x,y",
            "--at",
            "1.5,2",
            "--out",
            "out",
        ],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.path_refused");
    // Missing arguments are usage, not panics.
    common::assert_refusal(
        &differentiate(&dir, &["--function", "heat"]),
        "catalyst.usage",
    );
    let _ = heat;
}

#[test]
fn oracle_12_8_offline_by_construction_and_inside_the_engine() {
    // An empty environment: no PATH, no HOME, nothing.
    let dir = with_module("offline", "llvm/programs.ll");
    let run = common::run_with(
        &common::catalyst_bin(),
        &[
            "differentiate",
            "llvm",
            "--module",
            "module.ll",
            "--function",
            "heat",
            "--inputs",
            "x,y",
            "--at",
            "1.5,2",
            "--out",
            "out",
        ],
        &dir,
        true,
        &[],
        None,
        std::time::Duration::from_secs(common::PER_TEST_BOUND_SECS),
    );
    assert_eq!(
        run.status,
        Some(0),
        "12.8: works with an empty environment:\n{}",
        run.summary()
    );
    // A program that reaches outside the module is refused before it runs.
    let dir = common::scratch("opaque");
    common::write(
        &dir.join("module.ll"),
        "declare double @secret_source(double)\n\n\
         define double @leak(double %x) {\nstart:\n  %s = call double @secret_source(double %x)\n  %r = fmul double %s, %x\n  ret double %r\n}\n",
    );
    let refusal = common::assert_refusal(
        &differentiate(
            &dir,
            &[
                "--function",
                "leak",
                "--inputs",
                "x",
                "--at",
                "1",
                "--out",
                "out",
            ],
        ),
        "catalyst.opaque_call",
    );
    assert!(
        refusal.str_field("detail").contains("secret_source"),
        "12.8: names the callee"
    );
    // A loop that never ends for these inputs is a fault within the bound, not a hang.
    let dir = common::scratch("spin");
    common::write(
        &dir.join("module.ll"),
        "define double @spin(double %x) {\nstart:\n  br label %loop\nloop:\n  %acc = phi double [ %x, %start ], [ %next, %loop ]\n  %next = fadd double %acc, 1.000000e+00\n  %c = fcmp olt double %next, 0.000000e+00\n  br i1 %c, label %done, label %loop\ndone:\n  ret double %next\n}\n",
    );
    let bound = common::Bound::start("a non-terminating program is refused within the bound");
    let run = differentiate(
        &dir,
        &[
            "--function",
            "spin",
            "--inputs",
            "x",
            "--at",
            "0",
            "--out",
            "out",
        ],
    );
    common::assert_refusal(&run, "catalyst.llvm_run_faulted");
    bound.check();
    // And the same program with inputs that do terminate is fine.
    let run = differentiate(
        &dir,
        &[
            "--function",
            "spin",
            "--inputs",
            "x",
            "--at",
            "-2.5",
            "--out",
            "ok",
        ],
    );
    let json = common::assert_ok(&run);
    close(number(&json, "gradient.x"), 1.0, "spin d/dx");
    // The subsystem never touches a process, a socket, the environment or the filesystem itself.
    let files: Vec<(String, String)> = common::src_files()
        .into_iter()
        .filter(|(path, _)| path.starts_with("src/gradient_compiler/"))
        .collect();
    assert!(!files.is_empty(), "12.8: src/gradient_compiler/ exists");
    for (path, text) in files {
        for forbidden in ["process::Command", "std::net", "std::env", "std::fs"] {
            assert!(!text.contains(forbidden), "12.8: {path} uses {forbidden}");
        }
    }
}

fn v2(name: &str, computation: &str, inputs: &str, domains: &str) -> String {
    format!(
        "{{\"schema\":\"catalyst.problem.v2\",\"name\":\"{name}\",\"goal\":\"a computation, not only an expression\",\"computation\":{computation},\"inputs\":{inputs},\"domains\":{domains}}}"
    )
}

#[test]
fn oracle_12_9_a_problem_may_name_a_computation_of_more_than_one_kind() {
    let domains = r#"{"x":{"min":0.1,"max":10,"unit":""},"y":{"min":0.1,"max":10,"unit":""}}"#;
    // Kind llvm: a compiled program is a problem like any other.
    let dir = common::scratch("v2-llvm");
    common::write(&dir.join("heat.ll"), &common::fixture("llvm/programs.ll"));
    common::write(
        &dir.join("heat.json"),
        &v2(
            "heat",
            r#"{"kind":"llvm","module":"heat.ll","function":"heat"}"#,
            r#"{"x":1.5,"y":2}"#,
            domains,
        ),
    );
    let json = common::assert_ok(&common::catalyst(
        &["eval", "--problem", "heat.json"],
        &dir,
        &[],
    ));
    let (v, dx, dy) = heat(1.5, 2.0);
    close(number(&json, "value"), v, "v2 llvm value");
    close(number(&json, "gradient.x"), dx, "v2 llvm d/dx");
    close(number(&json, "gradient.y"), dy, "v2 llvm d/dy");
    assert_assurance(&json, "llvm", "cgc", false);
    assert_validation(&json, "validation", 5.0);
    // Its export is refused by name and writes nothing.
    for target in ["go", "r"] {
        let run = common::catalyst(
            &["export", target, "--problem", "heat.json", "--out", "exp"],
            &dir,
            &[],
        );
        common::assert_refusal(&run, "catalyst.portable_export_unavailable");
        assert!(
            !dir.join("exp").exists(),
            "12.9: a refused export writes nothing"
        );
    }
    // An integer parameter is held fixed through `computation.fixed`.
    common::write(
        &dir.join("cost.json"),
        &v2(
            "cost",
            r#"{"kind":"llvm","module":"heat.ll","function":"cost","fixed":{"n":5}}"#,
            r#"{"x":0.7}"#,
            r#"{"x":{"min":-3,"max":3,"unit":"rad"}}"#,
        ),
    );
    let json = common::assert_ok(&common::catalyst(
        &["eval", "--problem", "cost.json"],
        &dir,
        &[],
    ));
    let (v, d) = cost(0.7, 5);
    close(number(&json, "value"), v, "v2 cost value");
    close(number(&json, "gradient.x"), d, "v2 cost d/dx");
    common::write(
        &dir.join("cost-incomplete.json"),
        &v2(
            "cost",
            r#"{"kind":"llvm","module":"heat.ll","function":"cost"}"#,
            r#"{"x":0.7}"#,
            r#"{"x":{"min":-3,"max":3,"unit":"rad"}}"#,
        ),
    );
    common::assert_refusal(
        &common::catalyst(&["eval", "--problem", "cost-incomplete.json"], &dir, &[]),
        "catalyst.problem_incomplete",
    );
    // Kind catalyst-expression: exactly the v1 numbers, with the lane made visible.
    let dir = common::scratch("v2-expr");
    common::write(&dir.join("v1.json"), &common::simple_problem());
    common::write(
        &dir.join("v2.json"),
        &v2(
            "simple",
            r#"{"kind":"catalyst-expression","function":"func simple(x, y) = x * y + sin(x)"}"#,
            r#"{"x":0.7,"y":1.3}"#,
            r#"{"x":{"min":-3,"max":3,"unit":""},"y":{"min":-2,"max":2,"unit":""}}"#,
        ),
    );
    let one = common::assert_ok(&common::catalyst(
        &["eval", "--problem", "v1.json"],
        &dir,
        &[],
    ));
    let two = common::assert_ok(&common::catalyst(
        &["eval", "--problem", "v2.json"],
        &dir,
        &[],
    ));
    assert_eq!(number(&one, "value"), number(&two, "value"));
    assert_eq!(number(&one, "gradient.x"), number(&two, "gradient.x"));
    assert_eq!(number(&one, "gradient.y"), number(&two, "gradient.y"));
    assert!(
        one.get("assurance").is_none(),
        "12.9: a v1 document is unchanged in every way"
    );
    assert_assurance(&two, "catalyst-ir", "native", true);
    // A kind this phase does not build refuses and says what to do instead.
    common::write(
        &dir.join("rust.json"),
        &v2(
            "aero",
            r#"{"kind":"rust","source":"src/aero.rs","function":"drag"}"#,
            r#"{"x":1}"#,
            r#"{"x":{"min":0,"max":2,"unit":""}}"#,
        ),
    );
    let refusal = common::assert_refusal(
        &common::catalyst(&["eval", "--problem", "rust.json"], &dir, &[]),
        "catalyst.computation_kind_unsupported",
    );
    let remedy = refusal.str_field("remedy");
    assert!(
        remedy.contains("llvm") && remedy.contains("emit-llvm"),
        "12.9: the remedy says how to get LLVM IR:\n{remedy}"
    );
}

#[test]
fn oracle_12_10_the_subsystem_has_its_own_identity() {
    let this_file = "tests/acceptance_10_gradient_compiler.rs";
    let mut findings = Vec::new();
    for (path, text) in common::text_files(&common::ignored_names()) {
        if path == this_file {
            continue;
        }
        // Two adjacent lines at a time, because a sentence wraps: "a rebuild of
        // the pipeline" on one line and "Enzyme occupies" on the next is the
        // sentence this oracle exists for.
        let lines: Vec<String> = text.lines().map(str::to_ascii_lowercase).collect();
        for (number, line) in lines.iter().enumerate() {
            if line.contains("00-what-enzyme-is") {
                findings.push(format!(
                    "{path}:{} refers to a document that is not in the tree",
                    number + 1
                ));
            }
            let window = match lines.get(number + 1) {
                Some(next) => format!("{line} {next}"),
                None => line.clone(),
            };
            if window.contains("enzyme")
                && [
                    "rebuild",
                    "port of",
                    "clone of",
                    "reimplement",
                    "rewrite of",
                    "wrapper around",
                    "wrapper of",
                    "renamed",
                ]
                .iter()
                .any(|w| window.contains(w))
            {
                findings.push(format!(
                    "{path}:{} presents Catalyst as a rebuild of another project",
                    number + 1
                ));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "12.10: Catalyst is its own project:\n{}",
        findings.join("\n")
    );
    for path in ["README.md", "docs/interface.md", "src/lib.rs"] {
        let text = common::read(path).unwrap_or_else(|| panic!("12.10: {path} exists"));
        assert!(
            text.contains("Catalyst Gradient Compiler"),
            "12.10: {path} names the Catalyst Gradient Compiler"
        );
    }
    let root = common::read("src/gradient_compiler/mod.rs")
        .expect("12.10: src/gradient_compiler/mod.rs exists");
    assert!(
        root.contains("trait DifferentiationBackend"),
        "12.10: the backend trait is named as agreed"
    );
    assert!(
        root.contains("fn differentiate"),
        "12.10: the trait's method"
    );
    assert!(
        common::read("src/gradient_compiler/artifact.rs").is_some(),
        "12.10: the artifact has its own file"
    );
    let manifest = common::read("Cargo.toml").unwrap_or_default();
    let description = common::package_field(&manifest, "description").unwrap_or_default();
    assert!(
        !description.to_ascii_lowercase().contains("enzyme"),
        "12.10: the crate describes itself"
    );
}

#[test]
fn oracle_12_11_the_native_lane_produces_the_same_artifact_model() {
    let dir = common::scratch("native");
    common::write(&dir.join("simple.json"), &common::simple_problem());
    let eval = common::assert_ok(&common::catalyst(
        &["eval", "--problem", "simple.json"],
        &dir,
        &[],
    ));
    let run = common::catalyst(
        &[
            "differentiate",
            "native",
            "--problem",
            "simple.json",
            "--out",
            "out",
        ],
        &dir,
        &[],
    );
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("backend"), "native");
    assert_eq!(json.str_field("mode"), "reverse");
    assert_eq!(
        number(&json, "value"),
        number(&eval, "value"),
        "12.11: exactly eval's number"
    );
    assert_eq!(number(&json, "gradient.x"), number(&eval, "gradient.x"));
    assert_eq!(number(&json, "gradient.y"), number(&eval, "gradient.y"));
    assert_assurance(&json, "catalyst-ir", "native", true);
    assert_validation(&json, "validation", 5.0);
    let out = dir.join("out");
    let mut expected = vec![
        "derivative.cir",
        "derivative.json",
        "fixtures.json",
        "primal.cir",
        "problem.json",
        "validation-report.md",
    ];
    expected.sort();
    assert_eq!(common::listing(&out), expected);
    let doc = Json::parse(&common::read_file(&out.join("derivative.json"))).unwrap();
    assert_eq!(doc.str_field("backend"), "native");
    assert_eq!(
        doc.path("provenance.compiler").and_then(Json::as_str),
        Some("catalyst")
    );
    let function = Json::parse(&common::read_file(&out.join("problem.json")))
        .unwrap()
        .str_field("function")
        .to_owned();
    assert_eq!(
        doc.str_field("source_digest"),
        common::sha256_hex(function.as_bytes()),
        "12.11: source is the function text"
    );
    assert_eq!(
        doc.str_field("compiler_ir_digest"),
        common::sha256_hex(common::read_file(&out.join("primal.cir")).as_bytes()),
        "12.11: the compiler IR of the native lane is the primal"
    );
    assert!(common::read_file(&out.join("validation-report.md")).contains(SENTENCE));
    let run = common::catalyst(
        &[
            "differentiate",
            "run",
            "--artifact",
            "out",
            "--at",
            "0.7,1.3",
        ],
        &dir,
        &[],
    );
    let again = common::assert_ok(&run);
    assert_eq!(
        again.get("artifact_verified").and_then(Json::as_bool),
        Some(true)
    );
    assert_eq!(number(&again, "value"), number(&eval, "value"));
    assert_eq!(number(&again, "gradient.x"), number(&eval, "gradient.x"));
}

#[test]
fn oracle_12_12_the_tool_surface_serves_the_capability_and_writes_nothing() {
    let dir = common::scratch("tool");
    let discover = common::assert_ok(&common::catalyst(&["tools", "discover"], &dir, &[]));
    let tools: Vec<&str> = discover
        .get("tools")
        .and_then(Json::as_arr)
        .expect("12.12: tools")
        .iter()
        .map(|t| t.str_field("name"))
        .collect();
    assert_eq!(
        tools,
        [
            "discover",
            "validate_problem",
            "evaluate",
            "export_go",
            "differentiate_llvm"
        ],
        "12.12: the list is five"
    );
    let module = common::quote(&common::fixture("llvm/programs.ll"));
    let request = format!(
        r#"{{"schema":"catalyst.tool-request.v1","tool":"differentiate_llvm","arguments":{{"module":{module},"function":"heat","inputs":["x","y"],"at":[1.5,2]}}}}"#
    );
    common::write(&dir.join("request.json"), &request);
    let before = common::listing(&dir);
    let json = common::assert_ok(&common::catalyst(
        &["tools", "call", "--request", "request.json"],
        &dir,
        &[],
    ));
    assert_eq!(json.str_field("schema"), "catalyst.tool-response.v1");
    assert_eq!(json.str_field("tool"), "differentiate_llvm");
    let (v, dx, dy) = heat(1.5, 2.0);
    close(number(&json, "result.value"), v, "tool value");
    close(number(&json, "result.gradient.x"), dx, "tool d/dx");
    close(number(&json, "result.gradient.y"), dy, "tool d/dy");
    assert_eq!(
        json.path("result.backend").and_then(Json::as_str),
        Some("cgc")
    );
    let result = json.get("result").expect("12.12: result");
    assert_assurance(result, "llvm", "cgc", false);
    assert_validation(result, "validation", 5.0);
    assert_eq!(
        common::listing(&dir),
        before,
        "12.12: the tool wrote nothing"
    );
    // The verifier's tolerance is not the model's to set.
    let request = format!(
        r#"{{"schema":"catalyst.tool-request.v1","tool":"differentiate_llvm","arguments":{{"module":{module},"function":"heat","inputs":["x","y"],"at":[1.5,2],"tolerance":1}}}}"#
    );
    common::write(&dir.join("authority.json"), &request);
    common::assert_refusal(
        &common::catalyst(&["tools", "call", "--request", "authority.json"], &dir, &[]),
        "catalyst.authority_refused",
    );
    // The adapter boundary still lists exactly the tools the surface serves.
    let conformance = common::catalyst(&["adapter", "conformance"], &dir, &[]);
    let json = common::assert_ok(&conformance);
    assert_eq!(json.str_field("label"), common::LABEL);
}

#[test]
fn oracle_12_13_the_command_is_documented_by_the_binary_itself() {
    let dir = common::scratch("help");
    let top = common::catalyst(&["--help"], &dir, &[]);
    assert_eq!(top.status, Some(0));
    assert!(
        top.stdout.contains("differentiate"),
        "12.13: `catalyst --help` lists differentiate"
    );
    let help = common::catalyst(&["differentiate", "--help"], &dir, &[]);
    assert_eq!(help.status, Some(0), "12.13:\n{}", help.summary());
    for word in [
        "llvm",
        "native",
        "run",
        "--describe",
        "--module",
        "--function",
        "--inputs",
        "--at",
        "--out",
        "--mode",
        "--rules",
        "--source",
        "--artifact",
    ] {
        assert!(
            help.stdout.contains(word),
            "12.13: `catalyst differentiate --help` documents {word}"
        );
    }
}
