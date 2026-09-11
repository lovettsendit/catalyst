//! The R export (§9). Catalyst's promise is a checked, self-contained export;
//! Go was the first language it could be handed to and R is the second. The
//! two exports of one problem must agree on the arithmetic, and the R one must
//! run under plain `Rscript --vanilla` with no package installed, because a
//! reader who is handed it cannot be asked to install anything.
//!
//! R is disclosed infrastructure on this host, exactly as Go and WezTerm are.
mod acceptance_common;
use acceptance_common as common;
use common::Json;
use std::path::Path;
use std::process::Command;

/// The files an R export always contains.
const R_FILES: [&str; 5] = [
    "function.R",
    "parameters.R",
    "fixtures.R",
    "test_function.R",
    "validation-report.md",
];

/// Run `Rscript --vanilla` on a file with a driver, from `cwd`.
fn rscript(cwd: &Path, driver: &str, file: &str) -> (bool, String, String) {
    let out = Command::new("Rscript")
        .args(["--vanilla", "-e", driver, file])
        .current_dir(cwd)
        .output()
        .expect(
            "R is disclosed infrastructure for this build, as Go and WezTerm are; \
             `Rscript` must be on PATH",
        );
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// CanaryIO's own R parse driver, so the export is checked the way the host
/// checks R and not by a rule invented here.
const PARSE_DRIVER: &str = "invisible(parse(commandArgs(trailingOnly = TRUE)[1]))";

/// CanaryIO's own R test driver: source the file, call every zero-argument
/// `test_*` function, and announce how many were called.
const TEST_DRIVER: &str = r#"path <- commandArgs(trailingOnly = TRUE)[1]
source(path)
cases <- sort(Filter(function(name) startsWith(name, "test_") && is.function(get(name)) && length(formals(get(name))) == 0L, ls()))
cat(sprintf("ran %d cases\n", length(cases)))
for (case in cases) get(case)()
"#;

fn export_spring(dir: &Path, out: &str) -> common::Run {
    common::write(&dir.join("spring.json"), &common::spring_problem());
    common::catalyst(
        &["export", "r", "--problem", "spring.json", "--out", out],
        dir,
        &[],
    )
}

#[test]
fn oracle_8_1_an_r_export_is_written_and_is_self_contained() {
    let dir = common::scratch("r-export");
    let run = export_spring(&dir, "export/spring-r");
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("out"), "export/spring-r");
    let files: Vec<String> = json
        .get("files")
        .and_then(Json::as_arr)
        .expect("8.1: the export names its files")
        .iter()
        .map(|f| f.as_str().unwrap_or_default().to_owned())
        .collect();
    for name in R_FILES {
        assert!(
            files.iter().any(|f| f == name),
            "8.1: the export must name {name}, it named {files:?}"
        );
        assert!(
            dir.join("export/spring-r").join(name).is_file(),
            "8.1: {name} must be on disk"
        );
    }
    // Self-contained means base R only: no package is loaded, by any spelling.
    for name in [
        "function.R",
        "parameters.R",
        "fixtures.R",
        "test_function.R",
    ] {
        let text = common::read_file(&dir.join("export/spring-r").join(name));
        for forbidden in ["library(", "require(", "requireNamespace(", "::"] {
            assert!(
                !text.contains(forbidden),
                "8.1: {name} must need no package, it uses `{forbidden}`"
            );
        }
    }
}

#[test]
fn oracle_8_2_the_exported_r_parses_and_its_own_tests_pass() {
    let dir = common::scratch("r-runs");
    common::assert_ok(&export_spring(&dir, "export/spring-r"));
    let out = dir.join("export/spring-r");
    // Every file parses, under the host's own parse driver.
    for name in [
        "function.R",
        "parameters.R",
        "fixtures.R",
        "test_function.R",
    ] {
        let (ok, _, stderr) = rscript(&out, PARSE_DRIVER, name);
        assert!(ok, "8.2: {name} must parse as R:\n{stderr}");
    }
    // The export's own test file runs and every case passes.
    let (ok, stdout, stderr) = rscript(&out, TEST_DRIVER, "test_function.R");
    assert!(
        ok,
        "8.2: the exported R tests must pass:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let ran = stdout
        .lines()
        .find_map(|l| l.strip_prefix("ran ").and_then(|r| r.split(' ').next()))
        .and_then(|n| n.parse::<usize>().ok())
        .expect("8.2: the driver announces how many cases it called");
    assert!(
        ran >= 2,
        "8.2: a test file that defines fewer than two cases is not a check, it called {ran}"
    );
}

#[test]
fn oracle_8_3_the_r_and_go_exports_of_one_problem_agree() {
    let dir = common::scratch("r-go-parity");
    common::write(&dir.join("spring.json"), &common::spring_problem());
    common::assert_ok(&common::catalyst(
        &["export", "r", "--problem", "spring.json", "--out", "r-out"],
        &dir,
        &[],
    ));
    common::assert_ok(&common::catalyst(
        &[
            "export",
            "go",
            "--problem",
            "spring.json",
            "--out",
            "go-out",
        ],
        &dir,
        &[],
    ));
    // The Go export's fixtures are the reference: the same cases, the same
    // numbers, written for a different reader.
    let go_fixtures = Json::parse(&common::read_file(&dir.join("go-out/fixtures.json")))
        .expect("8.3: the Go fixtures");
    let cases = go_fixtures
        .get("cases")
        .and_then(Json::as_arr)
        .expect("8.3: the Go fixtures list cases");
    let r_fixtures = common::read_file(&dir.join("r-out/fixtures.R"));
    assert!(
        !cases.is_empty(),
        "8.3: a fixture set with no case proves nothing"
    );
    for case in cases {
        let value = case.num_field("value");
        // Rust and R both print a double shortest-round-trip, so the digits a
        // fixture carries are the digits the other export carries.
        let printed = format!("{value}");
        assert!(
            r_fixtures.contains(&printed),
            "8.3: the R fixtures must carry the same expected value {printed}"
        );
    }
    // And R's own run of those fixtures agrees with the numbers Go was given.
    let (ok, stdout, stderr) = rscript(&dir.join("r-out"), TEST_DRIVER, "test_function.R");
    assert!(ok, "8.3: R must reproduce them:\n{stdout}\n{stderr}");
}

#[test]
fn oracle_8_4_the_r_export_obeys_the_path_rule_and_the_problem_rules() {
    let dir = common::scratch("r-refusals");
    common::write(&dir.join("spring.json"), &common::spring_problem());
    // The same path rule as every other writer: no escape, no absolute path
    // outside the working directory, no symbolic link.
    let run = common::catalyst(
        &[
            "export",
            "r",
            "--problem",
            "spring.json",
            "--out",
            "../away",
        ],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.path_refused");
    assert!(
        !dir.parent().unwrap().join("away").exists(),
        "8.4: a refused export writes nothing"
    );
    // A problem Catalyst refuses is refused here with its own code, not a new one.
    common::write(
        &dir.join("broken.json"),
        r#"{"schema":"catalyst.problem.v1","name":"b","goal":"","function":"func b(x) = x +","inputs":{"x":1.0},"domains":{"x":{"min":0.0,"max":2.0,"unit":"m"}}}"#,
    );
    let run = common::catalyst(
        &["export", "r", "--problem", "broken.json", "--out", "out"],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.");
    assert!(
        !dir.join("out").exists(),
        "8.4: a refused export writes nothing"
    );
}

#[test]
fn oracle_8_5_the_r_export_carries_an_honest_validation_report() {
    let dir = common::scratch("r-report");
    common::assert_ok(&export_spring(&dir, "out"));
    let report = common::read_file(&dir.join("out/validation-report.md"));
    for forbidden in [
        "failure-free",
        "guarantee",
        "guaranteed",
        "will never fail",
        "will not fail",
        "cannot fail",
        "proven correct",
    ] {
        assert!(
            !report.to_lowercase().contains(forbidden),
            "8.5: the report must not claim `{forbidden}`"
        );
    }
    assert!(
        report.contains("Remaining uncertainty"),
        "8.5: the report says what it did not check"
    );
    assert!(
        !report.contains(&dir.to_string_lossy().into_owned()),
        "8.5: no absolute path from this machine reaches the export"
    );
}

#[test]
fn oracle_8_6_catalysts_own_r_tooling_checks_an_export() {
    // The `R/` directory is Catalyst's own R side: base-R helpers that take an
    // export directory and check it. This is the code the host's R lane runs.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("R");
    assert!(
        root.join("catalyst.R").is_file(),
        "8.6: Catalyst's R helpers live at R/catalyst.R"
    );
    assert!(
        root.join("test_catalyst.R").is_file(),
        "8.6: and are themselves checked, at R/test_catalyst.R"
    );
    let dir = common::scratch("r-tooling");
    common::assert_ok(&export_spring(&dir, "out"));
    // The helper checks the export that was just written, and says how many
    // cases it checked.
    let driver = format!(
        "source({});\ncat(sprintf(\"checked %d cases\\n\", catalyst_check_export({})))",
        common::quote(&root.join("catalyst.R").to_string_lossy()),
        common::quote(&dir.join("out").to_string_lossy())
    );
    let out = Command::new("Rscript")
        .args(["--vanilla", "-e", &driver])
        .current_dir(&dir)
        .output()
        .expect("R is disclosed infrastructure for this build");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "8.6: the helper must check a good export without complaint:\n{stdout}\n{stderr}"
    );
    let checked = stdout
        .lines()
        .find_map(|l| l.strip_prefix("checked ").and_then(|r| r.split(' ').next()))
        .and_then(|n| n.parse::<usize>().ok())
        .expect("8.6: the helper reports how many cases it checked");
    assert!(
        checked >= 2,
        "8.6: it must actually check the fixtures, it checked {checked}"
    );
}

#[test]
fn oracle_8_7_the_r_tooling_refuses_a_broken_export_rather_than_passing_it() {
    // A checker that cannot fail is not a checker.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("R");
    let dir = common::scratch("r-tooling-red");
    common::assert_ok(&export_spring(&dir, "out"));
    // Corrupt one expected value in the fixtures, leaving valid R.
    let fixtures = common::read_file(&dir.join("out/fixtures.R"));
    let broken = fixtures.replacen("value = ", "value = 1e9 + ", 1);
    assert_ne!(
        fixtures, broken,
        "8.7: the fixtures must carry a `value = ` to corrupt"
    );
    common::write(&dir.join("out/fixtures.R"), &broken);
    let driver = format!(
        "source({});\ncatalyst_check_export({})",
        common::quote(&root.join("catalyst.R").to_string_lossy()),
        common::quote(&dir.join("out").to_string_lossy())
    );
    let out = Command::new("Rscript")
        .args(["--vanilla", "-e", &driver])
        .current_dir(&dir)
        .output()
        .expect("R is disclosed infrastructure for this build");
    assert!(
        !out.status.success(),
        "8.7: a corrupted fixture must stop the check, it passed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("case"),
        "8.7: the refusal must name the case that disagreed:\n{stderr}"
    );
}

#[test]
fn oracle_8_8_erf_reaches_r_exactly_rather_than_being_refused() {
    // `erf` is what statistical, diffusion and process-yield problems are made
    // of. Base R has no `erf`, but it has `pnorm`, which `Rscript --vanilla`
    // provides with no package and no `::`, and
    //
    //     erf(x) = 2 * pnorm(x * sqrt(2)) - 1
    //
    // is an identity rather than an approximation. Measured against the engine
    // at x = 0.1, 0.5, 1.0, 2.0 and 3.5, the largest relative disagreement is
    // about 4e-16, six orders of magnitude inside the declared 1e-9. Refusing
    // `erf` would shut a whole class of users out of R for no measured reason.
    let dir = common::scratch("r-erf");
    common::write(
        &dir.join("erf.json"),
        r#"{"schema":"catalyst.problem.v1","name":"first-pass-yield",
            "goal":"keep first-pass yield above 99%",
            "function":"func fpy(mu, sigma, spec) = 0.5 * (1 + erf((spec - mu) / (sigma * sqrt(2))))",
            "inputs":{"mu":100.0,"sigma":1.2,"spec":104.0},
            "domains":{"mu":{"min":90.0,"max":110.0,"unit":"nm"},
                       "sigma":{"min":0.2,"max":4.0,"unit":"nm"},
                       "spec":{"min":100.5,"max":115.0,"unit":"nm"}}}"#,
    );
    let run = common::catalyst(
        &["export", "r", "--problem", "erf.json", "--out", "out"],
        &dir,
        &[],
    );
    common::assert_ok(&run);
    let out = dir.join("out");
    let function = common::read_file(&out.join("function.R"));
    for forbidden in ["library(", "require(", "requireNamespace(", "::"] {
        assert!(
            !function.contains(forbidden),
            "8.8: the erf export must still need no package, it uses `{forbidden}`"
        );
    }
    // The export's own fixtures are the check: R must reproduce the engine's
    // numbers within the tolerance the export itself declares.
    let (ok, stdout, stderr) = rscript(&out, TEST_DRIVER, "test_function.R");
    assert!(
        ok,
        "8.8: R must reproduce the engine on an erf problem:\n{stdout}\n{stderr}"
    );
    // And the Go export of the same problem agrees, so neither backend is
    // quietly using a different erf from the engine.
    common::assert_ok(&common::catalyst(
        &["export", "go", "--problem", "erf.json", "--out", "go-out"],
        &dir,
        &[],
    ));
    let go_fixtures = Json::parse(&common::read_file(&dir.join("go-out/fixtures.json")))
        .expect("8.8: the Go fixtures");
    let cases = go_fixtures
        .get("cases")
        .and_then(Json::as_arr)
        .expect("8.8: the Go fixtures list cases");
    let r_fixtures = common::read_file(&out.join("fixtures.R"));
    for case in cases {
        let printed = format!("{}", case.num_field("value"));
        assert!(
            r_fixtures.contains(&printed),
            "8.8: the R fixtures must carry the same expected value {printed}"
        );
    }
}
