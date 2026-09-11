//! Outcome 1 — a numerical goal becomes an independently checked Go export
//! (criteria 1.1–1.6 and the Go half of C11). Every file the export produces
//! is written under `$TMPDIR`, compiled there with `CGO_ENABLED=0` and caches
//! under `$TMPDIR`, and run there; the Rust engine's own numbers are
//! recomputed here through `catalyst::api::gradient` so the fixtures are
//! checked, not trusted.
mod acceptance_common;
use acceptance_common as common;
use common::{Json, Run};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const FILES: &[&str] = &[
    "go.mod",
    "function.go",
    "main.go",
    "parameters.json",
    "fixtures.json",
    "validation-report.md",
];
const FORBIDDEN_IMPORTS: &[&str] = &["unsafe", "C", "os/exec", "net", "syscall"];

struct Export {
    dir: PathBuf,
    problem: String,
    function: String,
    build: Run,
    vet: Run,
    output: Option<Run>,
}

fn export(name: &str, problem: String) -> Export {
    let bound = common::Bound::start("export, vet, build and run of one Go export");
    let dir = common::scratch(&format!("export-{name}"));
    common::write(&dir.join("problem.json"), &problem);
    let run = common::catalyst(
        &["export", "go", "--problem", "problem.json", "--out", "out"],
        &dir,
        &[],
    );
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("out"), "out");
    let out = dir.join("out");
    let function = Json::parse(&problem)
        .unwrap()
        .str_field("function")
        .to_string();
    let vet = common::go(&["vet", "./..."], &out);
    let build = common::go(&["build", "-o", "exported-binary", "."], &out);
    let output = if build.status == Some(0) {
        Some(common::run_with(
            &out.join("exported-binary"),
            &["fixtures.json"],
            &out,
            false,
            &[],
            None,
            std::time::Duration::from_secs(30),
        ))
    } else {
        None
    };
    bound.check();
    Export {
        dir: out,
        problem,
        function,
        build,
        vet,
        output,
    }
}

fn spring() -> &'static Export {
    static E: OnceLock<Export> = OnceLock::new();
    E.get_or_init(|| export("spring", common::spring_problem()))
}

fn simple() -> &'static Export {
    static E: OnceLock<Export> = OnceLock::new();
    E.get_or_init(|| export("simple", common::simple_problem()))
}

fn each() -> [&'static Export; 2] {
    [spring(), simple()]
}

fn fixtures(e: &Export) -> Json {
    Json::parse(&common::read_file(&e.dir.join("fixtures.json"))).expect("fixtures.json is JSON")
}

#[test]
fn oracle_1_1_export_is_a_standard_library_only_module_that_vets_and_builds() {
    for e in each() {
        for file in FILES {
            assert!(
                e.dir.join(file).is_file(),
                "1.2: the export must contain {file} (have: {:?})",
                common::listing(&e.dir)
            );
        }
        let go_mod = common::read_file(&e.dir.join("go.mod"));
        assert!(
            go_mod.lines().any(|l| l.starts_with("module ")),
            "1.1: go.mod names a module"
        );
        assert!(
            !go_mod.contains("require"),
            "1.1: go.mod must have no require entries:\n{go_mod}"
        );
        for file in ["function.go", "main.go"] {
            let source = common::read_file(&e.dir.join(file));
            for import in common::go_imports(&source) {
                assert!(
                    !FORBIDDEN_IMPORTS.contains(&import.as_str()),
                    "C11: {file} imports `{import}`"
                );
                let first = import.split('/').next().unwrap_or("");
                assert!(
                    !first.contains('.'),
                    "1.1: {file} imports `{import}`, which is not standard library"
                );
            }
        }
        assert_eq!(
            e.vet.status,
            Some(0),
            "1.1: go vet must pass:\n{}",
            e.vet.summary()
        );
        assert_eq!(
            e.build.status,
            Some(0),
            "1.1/1.6: go build with CGO_ENABLED=0 must pass:\n{}",
            e.build.summary()
        );
    }
}

#[test]
fn oracle_1_2_export_carries_parameters_domains_units_limitations_fixtures_and_report() {
    for e in each() {
        let params = Json::parse(&common::read_file(&e.dir.join("parameters.json")))
            .expect("parameters.json is JSON");
        assert_eq!(params.str_field("schema"), "catalyst.export-parameters.v1");
        let problem = Json::parse(&e.problem).unwrap();
        assert_eq!(params.str_field("name"), problem.str_field("name"));
        assert_eq!(params.str_field("function"), problem.str_field("function"));
        assert_eq!(params.str_field("goal"), problem.str_field("goal"));
        let names: Vec<&str> = problem
            .get("inputs")
            .and_then(Json::as_obj)
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        for n in &names {
            assert!(
                params.path(&format!("inputs.{n}")).is_some(),
                "1.2: parameters.json carries input {n}"
            );
            assert!(
                params.path(&format!("domains.{n}.min")).is_some(),
                "1.2: parameters.json carries the domain of {n}"
            );
            assert!(
                params
                    .path(&format!("units.{n}"))
                    .and_then(Json::as_str)
                    .is_some(),
                "1.2: parameters.json carries the unit of {n}"
            );
        }
        let limitations = params
            .get("limitations")
            .and_then(Json::as_arr)
            .expect("1.2: parameters.json carries limitations");
        assert!(
            limitations
                .iter()
                .any(|l| l.as_str() == Some(common::LIMITATION_GO)),
            "1.5: limitations must carry verbatim: {}",
            common::LIMITATION_GO
        );
        assert!(
            limitations
                .iter()
                .any(|l| l.as_str() == Some(common::LIMITATION_FLOAT)),
            "1.5: limitations must carry verbatim: {}",
            common::LIMITATION_FLOAT
        );
        let fx = fixtures(e);
        assert_eq!(fx.str_field("schema"), "catalyst.fixtures.v1");
        assert!(
            common::within(
                fx.path("tolerance.relative")
                    .and_then(Json::as_f64)
                    .unwrap(),
                1e-9,
                0.0,
                0.0
            ),
            "1.4: the default relative tolerance is 1e-9"
        );
        assert!(
            common::within(
                fx.path("tolerance.absolute")
                    .and_then(Json::as_f64)
                    .unwrap(),
                1e-12,
                0.0,
                0.0
            ),
            "1.4: the default absolute tolerance is 1e-12"
        );
        let report = common::read_file(&e.dir.join("validation-report.md"));
        assert!(
            report.contains(&problem.str_field("function").to_string()),
            "the report states the problem"
        );
        assert!(
            report.contains("1e-9") || report.contains("1e-09"),
            "the report states the tolerance"
        );
    }
}

#[test]
fn oracle_1_3_fixtures_cover_normal_boundary_and_out_of_domain_cases() {
    for e in each() {
        let problem = Json::parse(&e.problem).unwrap();
        let fx = fixtures(e);
        let cases = fx.get("cases").and_then(Json::as_arr).expect("cases");
        let kind = |c: &Json| c.str_field("kind").to_string();
        let normal = cases.iter().filter(|c| kind(c) == "normal").count();
        assert!(
            normal >= 3,
            "1.3: at least three normal cases (have {normal})"
        );
        for (name, domain) in problem.get("domains").and_then(Json::as_obj).unwrap() {
            let lo = domain.num_field("min");
            let hi = domain.num_field("max");
            for (edge, bound) in [("min", lo), ("max", hi)] {
                let case = cases
                    .iter()
                    .find(|c| c.str_field("name") == format!("boundary-{name}-{edge}"))
                    .unwrap_or_else(|| {
                        panic!(
                            "1.3: a boundary case boundary-{name}-{edge} for every declared domain"
                        )
                    });
                assert_eq!(kind(case), "boundary");
                let at = case
                    .path(&format!("inputs.{name}"))
                    .and_then(Json::as_f64)
                    .unwrap();
                assert!(
                    common::within(at, bound, 0.0, 0.0),
                    "1.3: boundary-{name}-{edge} puts {name} at {bound}, not {at}"
                );
            }
        }
        let outside: Vec<&Json> = cases
            .iter()
            .filter(|c| kind(c) == "out_of_domain")
            .collect();
        assert!(!outside.is_empty(), "1.3: at least one out-of-domain case");
        for c in outside {
            assert_eq!(
                c.path("expected.refused").and_then(Json::as_bool),
                Some(true),
                "1.3: the documented behaviour of an out-of-domain case is a refusal"
            );
        }
        for c in cases {
            if kind(c) == "boundary" {
                if let Some(t) = c.get("tolerance") {
                    assert!(
                        t.num_field("relative") >= 1e-9 && t.num_field("absolute") >= 1e-12,
                        "1.4: a case tolerance may only widen"
                    );
                    assert!(
                        t.str_field("reason").len() >= 8,
                        "1.4: a wider tolerance states its reason"
                    );
                }
            }
        }
    }
}

#[test]
fn oracle_1_4_the_compiled_go_matches_the_rust_engine_within_tolerance() {
    for e in each() {
        let output = e.output.as_ref().unwrap_or_else(|| {
            panic!(
                "1.1 first: the export did not build:\n{}",
                e.build.summary()
            )
        });
        assert_eq!(
            output.status,
            Some(0),
            "the exported binary runs the fixtures:\n{}",
            output.summary()
        );
        let results = Json::parse(output.stdout.trim()).unwrap_or_else(|err| {
            panic!(
                "the exported binary prints one JSON array ({err}):\n{}",
                output.summary()
            )
        });
        let results = results.as_arr().expect("a JSON array");
        let fx = fixtures(e);
        let rel = fx
            .path("tolerance.relative")
            .and_then(Json::as_f64)
            .unwrap();
        let abs = fx
            .path("tolerance.absolute")
            .and_then(Json::as_f64)
            .unwrap();
        let parsed = catalyst::text::parse(&e.function).expect("the function parses");
        for case in fx.get("cases").and_then(Json::as_arr).unwrap() {
            let name = case.str_field("name");
            let got = results
                .iter()
                .find(|r| r.get("case").and_then(Json::as_str) == Some(name))
                .unwrap_or_else(|| panic!("the Go output has no result for case {name}"));
            let at: Vec<f64> = parsed
                .params
                .iter()
                .map(|p| {
                    case.path(&format!("inputs.{p}"))
                        .and_then(Json::as_f64)
                        .unwrap_or_else(|| panic!("case {name} has input {p}"))
                })
                .collect();
            match case.str_field("kind") {
                "out_of_domain" => {
                    let refused = got
                        .get("refused")
                        .and_then(Json::as_str)
                        .unwrap_or_else(|| {
                            panic!("1.3: the Go must refuse out-of-domain case {name}: {got:?}")
                        });
                    assert!(
                        refused.starts_with("out of domain"),
                        "case {name}: {refused}"
                    );
                }
                _ => {
                    let (crel, cabs) = match case.get("tolerance") {
                        Some(t) => (t.num_field("relative"), t.num_field("absolute")),
                        None => (rel, abs),
                    };
                    let rust = catalyst::api::gradient(&e.function, &at)
                        .expect("the engine evaluates the case");
                    let finite =
                        rust.value.is_finite() && rust.gradient.iter().all(|g| g.is_finite());
                    if !finite {
                        assert_eq!(case.path("expected.nonfinite").and_then(Json::as_bool), Some(true), "case {name}: the engine is non-finite here, so the fixture must say so");
                        assert_eq!(
                            got.get("nonfinite").and_then(Json::as_bool),
                            Some(true),
                            "case {name}: the Go must report nonfinite too: {got:?}"
                        );
                        continue;
                    }
                    let expected_value = case
                        .path("expected.value")
                        .and_then(Json::as_f64)
                        .unwrap_or_else(|| panic!("case {name} carries expected.value"));
                    assert!(common::within(expected_value, rust.value, 1e-12, 1e-12), "case {name}: the fixture's expected value {expected_value} is not the engine's {}", rust.value);
                    let go_value = got
                        .get("value")
                        .and_then(Json::as_f64)
                        .unwrap_or_else(|| panic!("case {name}: Go output carries value: {got:?}"));
                    assert!(common::within(go_value, rust.value, crel, cabs), "1.4: case {name}: Go value {go_value} vs Rust {} outside tolerance ({crel}, {cabs})", rust.value);
                    for (i, p) in parsed.params.iter().enumerate() {
                        let expected_g = case
                            .path(&format!("expected.gradient.{p}"))
                            .and_then(Json::as_f64)
                            .unwrap_or_else(|| panic!("case {name} carries expected.gradient.{p}"));
                        assert!(common::within(expected_g, rust.gradient[i], 1e-12, 1e-12), "case {name}: fixture partial d/d{p} {expected_g} is not the engine's {}", rust.gradient[i]);
                        let go_g = got
                            .path(&format!("gradient.{p}"))
                            .and_then(Json::as_f64)
                            .unwrap_or_else(|| {
                                panic!("case {name}: Go output carries gradient.{p}: {got:?}")
                            });
                        assert!(
                            common::within(go_g, rust.gradient[i], crel, cabs),
                            "1.4: case {name}: Go d/d{p} {go_g} vs Rust {} outside tolerance",
                            rust.gradient[i]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn oracle_1_5_the_validation_report_states_both_limitations_verbatim() {
    for e in each() {
        let report = common::read_file(&e.dir.join("validation-report.md"));
        assert!(
            report.contains(common::LIMITATION_GO),
            "1.5: the report must state verbatim: {}",
            common::LIMITATION_GO
        );
        assert!(
            report.contains(common::LIMITATION_FLOAT),
            "1.5: the report must state verbatim: {}",
            common::LIMITATION_FLOAT
        );
    }
}

#[test]
fn oracle_1_6_building_the_go_needs_neither_rust_nor_cgo() {
    for e in each() {
        for file in ["function.go", "main.go"] {
            let source = common::read_file(&e.dir.join(file));
            assert!(!source.contains("import \"C\""), "1.6: no cgo in {file}");
            assert!(
                !source.to_ascii_lowercase().contains("cargo"),
                "1.6: the Go side must not need Rust ({file} mentions cargo)"
            );
        }
        assert_eq!(
            e.build.status,
            Some(0),
            "1.6: built with CGO_ENABLED=0 and no Rust toolchain in the module:\n{}",
            e.build.summary()
        );
    }
}

#[test]
fn oracle_1_exports_are_deterministic_and_carry_no_timestamp_or_personal_path() {
    let dir = common::scratch("export-determinism");
    common::write(&dir.join("problem.json"), &common::spring_problem());
    common::assert_ok(&common::catalyst(
        &["export", "go", "--problem", "problem.json", "--out", "a"],
        &dir,
        &[],
    ));
    common::assert_ok(&common::catalyst(
        &["export", "go", "--problem", "problem.json", "--out", "b"],
        &dir,
        &[],
    ));
    for file in FILES {
        let a = std::fs::read(dir.join("a").join(file)).unwrap();
        let b = std::fs::read(dir.join("b").join(file)).unwrap();
        assert!(
            a == b,
            "4.2/1.2: {file} differs between two exports of the same problem"
        );
        let text = String::from_utf8_lossy(&a);
        // Built at run time so this oracle's own text carries no home path.
        let home_linux = ["/ho", "me/"].concat();
        let home_mac = ["/Us", "ers/"].concat();
        assert!(
            !text.contains(&home_linux) && !text.contains(&home_mac) && !text.contains("/tmp/"),
            "C4: {file} carries an absolute path"
        );
        assert!(
            !text.contains("202"),
            "{file} looks like it carries a date: exports must be timestamp-free"
        );
    }
    // Exporting into a non-empty directory is refused rather than overwriting.
    let run = common::catalyst(
        &["export", "go", "--problem", "problem.json", "--out", "a"],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.path_not_empty");
    let _ = Path::new("");
}
