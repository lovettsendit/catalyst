//! Go that lives in the repository (§11).
//!
//! Catalyst emits Go and rehearses Go, so a reader who opens the repository
//! expects to find Go in it. Two pieces belong there and neither is generated
//! per problem, so both can be checked in and checked by the Go toolchain
//! directly rather than only after Catalyst has written them out:
//!
//! - `go/standin/` — the rehearsal stand-in service. It was a string constant
//!   inside a Rust file, which meant `go vet` only ever saw it after Catalyst
//!   had written it to a temporary directory. The bytes are now a file, and
//!   Rust embeds that file, so what ships and what is checked are the same
//!   bytes by construction rather than by agreement.
//! - `go/catalyst/` — the sibling of `R/catalyst.R`: a package that takes an
//!   export directory and checks it, so someone handed an export can verify it
//!   with the Go they already have.
mod acceptance_common;
use acceptance_common as common;
use std::path::{Path, PathBuf};

fn tree() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("go")
}

/// Every import of a Go file, the same reading `acceptance_1` uses.
///
/// C11's list is written for **an export**: generated Go handed to someone who
/// must be able to run it without trusting it. A stand-in is not an export. It
/// is a service, and a service that listens imports `net` because there is no
/// other way to listen. `net` is therefore allowed here and banned in an
/// export, which is what C11 actually says, and the ban that matters for a
/// service -- no shelling out, no raw system calls, no cgo, no `unsafe` -- is
/// enforced across every file rather than one.
fn imports_are_allowed(source: &str, what: &str) {
    for import in common::go_imports(source) {
        assert!(
            !import.split('/').next().unwrap_or("").contains('.'),
            "11: {what} must use only the standard library, it imports {import}"
        );
        for forbidden in ["unsafe", "C", "os/exec", "syscall"] {
            assert!(
                import != forbidden,
                "C11: {what} must not import {forbidden}"
            );
        }
    }
}

/// Read every Go file in a directory, so a rule cannot be satisfied by moving
/// the awkward line into a file the oracle does not look at.
fn every_go_file(dir: &Path) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("11: read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".go"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                common::read_file(&e.path()),
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!found.is_empty(), "11: no Go in {}", dir.display());
    found
}

#[test]
fn oracle_11_1_the_stand_in_is_a_file_in_the_tree_not_a_string_in_a_rust_source() {
    let dir = tree().join("standin");
    for name in ["go.mod", "main.go"] {
        assert!(
            dir.join(name).is_file(),
            "11.1: the stand-in's {name} belongs in go/standin/, as a file"
        );
    }
    let files = every_go_file(&dir);
    assert!(
        files
            .iter()
            .any(|(_, text)| text.to_ascii_lowercase().contains("stand-in")),
        "11.1/2.9: the stand-in still names itself a stand-in"
    );
    for (name, text) in &files {
        imports_are_allowed(text, &format!("go/standin/{name}"));
    }
    // A service listens, so exactly one file may open a socket, and it must be
    // obvious which. Everything else in the directory is ordinary code.
    let listeners: Vec<&String> = files
        .iter()
        .filter(|(_, text)| common::go_imports(text).iter().any(|i| i == "net"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        listeners.len(),
        1,
        "11.1: exactly one file in go/standin may import net, these do: {listeners:?}"
    );
    assert!(
        !common::read_file(&dir.join("go.mod")).contains("require"),
        "11.1: the stand-in's module needs nothing"
    );
    // The Rust side must not carry a second copy of the program text. One
    // definition, embedded; two would drift.
    let rust =
        common::read_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rehearse/standin.rs"));
    assert!(
        rust.contains("include_str!"),
        "11.1: embed the file rather than restating it in Rust"
    );
    assert!(
        !rust.contains("package main"),
        "11.1: the Go program text must not also be inline in Rust"
    );
}

#[test]
fn oracle_11_2_the_in_tree_stand_in_vets_and_builds_where_it_lives() {
    // The point of moving it: the Go toolchain checks it in the repository,
    // not only after Catalyst has written a copy somewhere temporary.
    let dir = tree().join("standin");
    let vet = common::go(&["vet", "./..."], &dir);
    assert_eq!(
        vet.status,
        Some(0),
        "11.2: go vet on go/standin\n{}",
        vet.summary()
    );
    // Build to the scratch this check was handed, never into the tree.
    // ACCEPTANCE.md: every oracle that produces files does so under $TMPDIR, so
    // a verdict can never have altered the source it ruled on. An earlier
    // version of this oracle wrote an 8.8 MB executable into `go/standin/` on
    // every run, which is exactly what that rule exists to prevent.
    let scratch = common::scratch("go-standin-build");
    let out = scratch.join("standin-binary");
    let build = common::go(&["build", "-o", &out.to_string_lossy(), "."], &dir);
    assert_eq!(
        build.status,
        Some(0),
        "11.2: go build on go/standin\n{}",
        build.summary()
    );
    assert!(out.is_file(), "11.2: the build produced a binary");
    // And nothing was left behind in the tree.
    let stray: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".go") && n != "go.mod" && n != "go.sum")
        .collect();
    assert!(
        stray.is_empty(),
        "11.2: building must leave nothing in go/standin, it left {stray:?}"
    );
}

#[test]
fn oracle_11_3_what_catalyst_writes_is_what_the_repository_holds() {
    // Byte-identity, so the checked copy and the shipped copy cannot diverge.
    let dir = common::scratch("go-standin-identity");
    let run = common::catalyst(&["rehearse", "standin", "--out", "written"], &dir, &[]);
    common::assert_ok(&run);
    for name in ["go.mod", "main.go"] {
        let written = common::read_file(&dir.join("written").join(name));
        let in_tree = common::read_file(&tree().join("standin").join(name));
        assert_eq!(
            written, in_tree,
            "11.3: go/standin/{name} and what `rehearse standin` writes must be the same bytes"
        );
    }
}

#[test]
fn oracle_11_4_catalysts_own_go_checker_verifies_an_export() {
    // The sibling of `R/catalyst.R`. Someone handed an export can check it
    // with the Go they already have, and it reports how many cases it checked
    // rather than a bare pass, because "it passed" and "it checked nothing"
    // must not look the same.
    let dir = tree().join("catalyst");
    assert!(
        dir.join("go.mod").is_file(),
        "11.4: Catalyst's Go checker lives in go/catalyst/"
    );
    let has_test = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with("_test.go"))
        })
        .unwrap_or(false);
    assert!(
        has_test,
        "11.4: and is itself checked, by a _test.go beside it"
    );
    for (name, text) in every_go_file(&dir) {
        imports_are_allowed(&text, &format!("go/catalyst/{name}"));
        assert!(
            !common::go_imports(&text).iter().any(|i| i == "net"),
            "11.4: a checker opens no socket, and go/catalyst/{name} imports net"
        );
    }
    assert!(
        !common::read_file(&dir.join("go.mod")).contains("require"),
        "11.4: the checker needs nothing either"
    );
    let vet = common::go(&["vet", "./..."], &dir);
    assert_eq!(vet.status, Some(0), "11.4: go vet\n{}", vet.summary());
    let test = common::go(&["test", "./..."], &dir);
    assert_eq!(
        test.status,
        Some(0),
        "11.4: the checker's own tests must pass\n{}",
        test.summary()
    );
}

#[test]
fn oracle_11_5_the_go_checker_refuses_a_corrupted_export() {
    // A checker that cannot fail is not a checker -- the same rule R's side is
    // held to.
    let dir = common::scratch("go-checker-red");
    common::write(&dir.join("spring.json"), &common::spring_problem());
    common::assert_ok(&common::catalyst(
        &["export", "go", "--problem", "spring.json", "--out", "out"],
        &dir,
        &[],
    ));
    let checker = tree().join("catalyst");
    let good = common::go(
        &["run", ".", "-export", &dir.join("out").to_string_lossy()],
        &checker,
    );
    assert_eq!(
        good.status,
        Some(0),
        "11.5: a good export must check clean\n{}",
        good.summary()
    );
    assert!(
        good.stdout.contains("checked"),
        "11.5: it says how many cases it checked:\n{}",
        good.stdout
    );
    // Corrupt one expected value, leaving valid JSON.
    let fixtures = common::read_file(&dir.join("out/fixtures.json"));
    let broken = fixtures.replacen("\"value\":", "\"value\":1e9,\"was\":", 1);
    assert_ne!(
        fixtures, broken,
        "11.5: the fixtures must carry a value to corrupt"
    );
    common::write(&dir.join("out/fixtures.json"), &broken);
    let bad = common::go(
        &["run", ".", "-export", &dir.join("out").to_string_lossy()],
        &checker,
    );
    assert_ne!(
        bad.status,
        Some(0),
        "11.5: a corrupted fixture must stop the check, it passed"
    );
}
