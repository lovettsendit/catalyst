//! C4 — nothing personal, private or secret in any public artifact.
mod acceptance_common;
use acceptance_common as common;

/// Each pattern is named so a failure never echoes the matched content.
const PLAIN: &[(&str, &str)] = &[
    ("a macOS home path", "/Users/"),
    ("a Linux home path", "/home/"),
    (
        "the CanaryIO data directory",
        "Application Support/canaryio",
    ),
    ("CanaryIO private state", ".canaryio-private"),
    ("CanaryIO protected core", "protected-core"),
    ("a CanaryIO capability pack", "capability-packs"),
    ("a CanaryIO receipt", "receipt-v1"),
    ("an AWS access key id", "AKIA"),
    ("a private key block", "PRIVATE KEY-----"),
];

fn looks_like_secret_assignment(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    for key in [
        "api_key",
        "apikey",
        "api-key",
        "secret_key",
        "access_token",
        "auth_token",
        "password",
    ] {
        if let Some(index) = lower.find(key) {
            let rest = &lower[index + key.len()..];
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':')) {
                let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                let literal = value
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .count();
                if literal >= 16 {
                    return true;
                }
            }
        }
    }
    lower.contains("sk-")
        && lower.split("sk-").nth(1).is_some_and(|tail| {
            tail.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .count()
                >= 20
        })
}

#[test]
fn c4_no_public_artifact_carries_private_or_personal_material() {
    let skip = common::ignored_names();
    let mut findings = Vec::new();
    for (path, text) in common::text_files(&skip) {
        // This file is the one place the pattern list may appear; it is the
        // oracle, not an artifact. Rehearsal 2026-09-10 showed it flagging
        // its own source, so it skips exactly itself and nothing else.
        if path.ends_with("acceptance_c4_no_private_material.rs") {
            continue;
        }
        for (number, line) in text.lines().enumerate() {
            for (what, needle) in PLAIN {
                if line.contains(needle) {
                    findings.push(format!("{path}:{} contains {what}", number + 1));
                }
            }
            if looks_like_secret_assignment(line) {
                findings.push(format!(
                    "{path}:{} contains a credential-shaped assignment",
                    number + 1
                ));
            }
        }
    }
    assert!(findings.is_empty(), "C4: remove private or personal material from these places (content deliberately not shown):\n{}", findings.join("\n"));
}
