//! C15 — SECURITY.md says how to report and what the posture is.
mod acceptance_common;
use acceptance_common as common;

#[test]
fn c15_security_md_states_reporting_and_posture() {
    let text = common::read("SECURITY.md")
        .expect("C15: add SECURITY.md at the workspace root")
        .to_ascii_lowercase();
    let required: &[(&str, &[&str])] = &[
        ("how to report a problem", &["report"]),
        (
            "no third-party code",
            &[
                "third-party",
                "third party",
                "no dependencies",
                "zero dependencies",
            ],
        ),
        ("no unsafe code", &["unsafe"]),
        ("no network in the manual path", &["network"]),
        ("rehearsal on loopback only", &["loopback", "127.0.0.1"]),
        ("credential from the environment only", &["environment"]),
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter(|(_, any)| !any.iter().any(|w| text.contains(w)))
        .map(|(what, _)| *what)
        .collect();
    assert!(
        missing.is_empty(),
        "C15: SECURITY.md must also state, in plain words: {missing:?}"
    );
}
