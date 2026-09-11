//! C1 — no third-party code, and no way to pull any in.
mod acceptance_common;
use acceptance_common as common;

#[test]
fn c1_the_crate_has_no_dependencies_of_any_kind() {
    let manifest =
        common::read("Cargo.toml").expect("C1: Cargo.toml must exist at the workspace root");
    let deps = common::section_lines(&manifest, "[dependencies]")
        .expect("C1: Cargo.toml must declare an explicit, empty [dependencies] table");
    let entries: Vec<&str> = deps
        .iter()
        .copied()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    assert!(
        entries.is_empty(),
        "C1: [dependencies] must stay empty; remove these entries: {entries:?}"
    );
    for forbidden in ["[dev-dependencies]", "[build-dependencies]"] {
        assert!(
            common::section_lines(&manifest, forbidden).is_none(),
            "C1: remove the {forbidden} table; no dependencies of any kind are allowed"
        );
    }
    for line in manifest.lines() {
        let trimmed = line.trim();
        assert!(
            !(trimmed.starts_with("[dependencies.")
                || trimmed.starts_with("[dev-dependencies.")
                || trimmed.starts_with("[build-dependencies.")
                || trimmed.starts_with("[target.")),
            "C1: remove the dependency table `{trimmed}`; no dependencies of any kind are allowed"
        );
    }
    assert!(
        !common::root().join("build.rs").exists(),
        "C1: delete build.rs; a build script is not allowed"
    );
    assert!(
        common::package_field(&manifest, "build").is_none(),
        "C1: remove the `build` key from [package]"
    );
}

#[test]
fn c1_the_lockfile_names_only_this_crate() {
    let lock = common::read("Cargo.lock")
        .expect("C1: Cargo.lock must exist and be committed; the check lane cannot write it");
    let names: Vec<String> = lock
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("name = ")
                .map(|v| v.trim_matches('"').to_string())
        })
        .collect();
    assert!(
        !names.is_empty(),
        "C1: Cargo.lock names no package; regenerate it with `cargo generate-lockfile --offline`"
    );
    let foreign: Vec<&String> = names.iter().filter(|n| n.as_str() != "catalyst").collect();
    assert!(foreign.is_empty(), "C1: Cargo.lock must name only `catalyst`; remove the dependencies that brought in {foreign:?}");
}
