//! C13 — Catalyst writes only to the path the user named; `..`, absolute
//! paths outside the workspace and symlinked targets are refused; no
//! user-supplied string ever reaches a shell.
mod acceptance_common;
use acceptance_common as common;

fn problem_dir(name: &str) -> std::path::PathBuf {
    let dir = common::scratch(name);
    common::write(&dir.join("problem.json"), &common::simple_problem());
    dir
}

#[test]
fn c13_a_parent_traversal_is_refused_and_nothing_is_written() {
    let dir = problem_dir("c13-dotdot");
    let parent = dir.parent().expect("scratch parent").to_path_buf();
    // Sibling tests of this binary create their own `catalyst-oracle-*` scratch
    // directories in the same parent on parallel threads; only what this test
    // could have created is compared.
    let outside = |names: Vec<String>| -> Vec<String> {
        names
            .into_iter()
            .filter(|name| !name.starts_with("catalyst-oracle-"))
            .collect()
    };
    let before = outside(common::listing(&parent));
    for out in ["../escape", "sub/../../escape", "./../escape"] {
        let run = common::catalyst(
            &["export", "go", "--problem", "problem.json", "--out", out],
            &dir,
            &[],
        );
        common::assert_refusal(&run, "catalyst.path_refused");
    }
    assert_eq!(
        outside(common::listing(&parent)),
        before,
        "a refused path must not create anything outside the workspace"
    );
    assert!(!dir.join("sub").exists());
}

#[test]
fn c13_an_absolute_path_outside_the_workspace_is_refused() {
    let dir = problem_dir("c13-absolute");
    let outside =
        std::env::temp_dir().join(format!("catalyst-oracle-outside-{}", std::process::id()));
    let out = outside.to_string_lossy().into_owned();
    let run = common::catalyst(
        &["export", "go", "--problem", "problem.json", "--out", &out],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.path_refused");
    assert!(
        !outside.exists(),
        "the refused absolute path must not be created"
    );
    // An absolute path inside the workspace is the same place as a relative one and is accepted.
    // The rule compares against the resolved current directory, so resolve the scratch
    // directory first: a temp dir behind a symlink (macOS `/var` -> `/private/var`) would
    // otherwise be refused as outside.
    let inside = dir
        .canonicalize()
        .expect("resolve scratch directory")
        .join("inside-export");
    let run = common::catalyst(
        &[
            "export",
            "go",
            "--problem",
            "problem.json",
            "--out",
            &inside.to_string_lossy(),
        ],
        &dir,
        &[],
    );
    common::assert_ok(&run);
    assert!(inside.join("function.go").is_file());
}

#[test]
fn c13_a_symlinked_target_is_refused() {
    let dir = problem_dir("c13-symlink");
    let outside =
        std::env::temp_dir().join(format!("catalyst-oracle-linktarget-{}", std::process::id()));
    std::fs::create_dir_all(&outside).expect("create link target");
    std::os::unix::fs::symlink(&outside, dir.join("link")).expect("create symlink");
    for out in ["link", "link/export"] {
        let run = common::catalyst(
            &["export", "go", "--problem", "problem.json", "--out", out],
            &dir,
            &[],
        );
        common::assert_refusal(&run, "catalyst.path_refused");
    }
    assert!(
        common::listing(&outside).is_empty(),
        "nothing may be written through the symlink"
    );
    // The same rule for every other user-named output.
    let run = common::catalyst(
        &[
            "tui",
            "--headless",
            "--keys",
            "keys.txt",
            "--transcript",
            "link/t.txt",
        ],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.path_refused");
    let run = common::catalyst(
        &[
            "rehearse",
            "init",
            "--dir",
            "../r",
            "--target",
            "unix:s.sock",
        ],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.path_refused");
    assert!(common::listing(&outside).is_empty());
}

#[test]
fn c13_no_source_names_a_shell() {
    let mut hits = Vec::new();
    for (path, text) in common::src_files() {
        for (number, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for needle in [
                "\"sh\"",
                "\"bash\"",
                "\"zsh\"",
                "\"/bin/sh\"",
                "\"/bin/bash\"",
                "\"cmd\"",
                "\"cmd.exe\"",
                "\"sh -c\"",
                "\"-c\"",
                "system(",
            ] {
                if code.contains(needle) {
                    hits.push(format!("{path}:{} ({needle})", number + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "C13: no user-supplied string may reach a shell; remove the shell references at {hits:?}"
    );
}
