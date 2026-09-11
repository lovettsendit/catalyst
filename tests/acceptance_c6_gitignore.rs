//! C6 — the tree is publishable as it stands.
mod acceptance_common;
use acceptance_common as common;

const MUST_IGNORE: &[&str] = &["target", ".venv", "logs", ".env", ".DS_Store"];

#[test]
fn c6_gitignore_covers_build_output_environments_logs_and_os_files() {
    let text = common::read(".gitignore").expect("C6: add a .gitignore at the workspace root");
    let covered: Vec<String> = text
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches('/')
                .trim_end_matches('/')
                .to_string()
        })
        .collect();
    let missing: Vec<&str> = MUST_IGNORE
        .iter()
        .copied()
        .filter(|name| !covered.iter().any(|c| c == name))
        .collect();
    assert!(
        missing.is_empty(),
        "C6: add these lines to .gitignore: {missing:?}"
    );
}

#[test]
fn c6_no_stray_files_outside_ignored_paths() {
    let skip = common::ignored_names();
    let base = common::root();
    let stray: Vec<String> = common::walk(&skip)
        .into_iter()
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            name == ".DS_Store"
                || name.ends_with(".log")
                || name.ends_with(".orig")
                || name.ends_with(".rej")
                || name.ends_with('~')
                || name.ends_with(".tmp")
                || name.starts_with("._")
        })
        .map(|p| common::relative(&base, &p))
        .collect();
    assert!(
        stray.is_empty(),
        "C6: delete these stray files or ignore their directories: {stray:?}"
    );
}
