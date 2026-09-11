//! 5.3 / 5.5 — the fixed local-adapter label, and no claim beyond it.
mod acceptance_common;
use acceptance_common as common;

const LABEL: &str = "local-adapter extension interface; live local-model compatibility untested";

#[test]
fn label_appears_in_src_and_readme() {
    let readme = common::read("README.md").unwrap_or_default();
    assert!(
        readme.contains(LABEL),
        "5.3: README.md must carry the label verbatim: {LABEL}"
    );
    let skip = common::ignored_names();
    let in_src = common::text_files(&skip)
        .iter()
        .any(|(path, text)| path.starts_with("src/") && text.contains(LABEL));
    assert!(
        in_src,
        "5.3: at least one adapter surface under src/ must carry the label verbatim: {LABEL}"
    );
}

#[test]
fn no_claim_of_verified_local_model_support() {
    let skip = common::ignored_names();
    let mut claims = Vec::new();
    for (path, text) in common::text_files(&skip) {
        if !(path.starts_with("src/")
            || path.starts_with("docs/")
            || path == "README.md"
            || path == "SECURITY.md")
        {
            continue;
        }
        for (number, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            if !(lower.contains("local model") || lower.contains("local-model"))
                || lower.contains("untested")
            {
                continue;
            }
            if [
                "verified",
                "supported",
                "works with",
                "tested with",
                "compatible with",
                "supports ",
            ]
            .iter()
            .any(|w| lower.contains(w))
            {
                claims.push(format!("{path}:{}", number + 1));
            }
        }
    }
    assert!(claims.is_empty(), "5.5: remove the claim of verified local-model support at {claims:?}; only the fixed label is allowed");
}
