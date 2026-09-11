//! C7 — README.md is true: it says what Catalyst is, how to build and test it
//! offline, how to use it by hand with AI off, how to export Go and run the
//! export, and carries the local-adapter label; every `cargo` or `catalyst`
//! command it shows is one the crate accepts.
mod acceptance_common;
use acceptance_common as common;

fn readme() -> String {
    common::read("README.md").expect("C7: README.md must exist at the workspace root")
}

#[test]
fn c7_readme_says_what_catalyst_is_and_how_to_build_test_use_and_export() {
    let text = readme();
    let lower = text.to_ascii_lowercase();
    let required: &[(&str, &[&str])] = &[
        ("what Catalyst is (a heading naming it)", &["# catalyst"]),
        ("how to build offline", &["cargo build --offline"]),
        ("how to test offline", &["cargo test --offline"]),
        (
            "manual use with the AI off",
            &["ai off", "ai switched off", "without the ai", "ai is off"],
        ),
        ("how to export Go", &["catalyst export go"]),
        ("how to run the export", &["go build", "go run"]),
        ("the local-adapter label", &[common::LABEL]),
        ("the licence", &["server side public license"]),
        ("how to report a security problem", &["security.md"]),
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter(|(_, any)| !any.iter().any(|w| lower.contains(&w.to_ascii_lowercase())))
        .map(|(what, _)| *what)
        .collect();
    assert!(
        missing.is_empty(),
        "C7: README.md must also say: {missing:?}"
    );
    for stale in [
        "is planned, not implemented",
        "deliberately not being implemented",
        "not implemented in advance",
    ] {
        assert!(!lower.contains(stale), "C7: README.md still carries the pre-build wording `{stale}`; it must describe what exists now");
    }
}

#[test]
fn c7_every_catalyst_command_the_readme_shows_is_accepted() {
    let text = readme();
    let bytes = text.as_bytes();
    let mut subcommands = std::collections::BTreeSet::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find("catalyst ") {
        let at = start + pos;
        let before = if at == 0 { b' ' } else { bytes[at - 1] };
        start = at + "catalyst ".len();
        if before.is_ascii_alphanumeric()
            || before == b'/'
            || before == b'-'
            || before == b'.'
            || before == b'_'
        {
            continue;
        }
        let rest = &text[start..];
        let word: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '-')
            .collect();
        if word.is_empty() || word.starts_with('-') {
            continue;
        }
        // Prose such as "catalyst is" or "catalyst does": only words the binary could accept.
        let next = rest[word.len()..].chars().next().unwrap_or(' ');
        if !matches!(next, ' ' | '\n' | '`' | '\t') {
            continue;
        }
        subcommands.insert(word);
    }
    assert!(
        subcommands.contains("export"),
        "C7: README.md must show `catalyst export go`"
    );
    let prose = [
        "is",
        "does",
        "has",
        "was",
        "will",
        "can",
        "reads",
        "writes",
        "runs",
        "and",
        "or",
        "the",
        "a",
        "an",
        "in",
        "to",
        "with",
        "for",
        "of",
        "on",
        "by",
        "as",
        "so",
        "at",
        "its",
        "may",
        "must",
        "never",
        "only",
        "itself",
        "also",
        "ships",
        "refuses",
        "exports",
        "evaluates",
        "proposes",
        "rehearses",
        "already",
        "uses",
        "talks",
        "keeps",
        "makes",
        "needs",
        "stays",
        "takes",
        "then",
        "from",
        "into",
        "when",
        "which",
        "that",
        "this",
        "no",
        "not",
        "but",
        "own",
        "product",
        "library",
        "crate",
        "binary",
        "code",
        "help",
        "if",
        "it",
        "you",
        "we",
        "they",
        "your",
        "our",
        "all",
        "each",
        "every",
        "any",
        "one",
        "two",
        "three",
        "four",
        "five",
        "some",
        "more",
        "less",
        "than",
        "are",
        "were",
        "be",
        "been",
        "being",
        "do",
        "did",
        "done",
        "had",
        "have",
        "having",
        "goes",
        "went",
        "gets",
        "got",
        "gives",
        "gave",
        "sees",
        "saw",
        "shows",
        "showed",
        "prints",
        "records",
        "checks",
        "compares",
        "reports",
        "writes",
        "opens",
        "spawns",
        "requires",
        "expects",
        "returns",
        "accepts",
        "rejects",
        "validates",
        "generates",
        "derives",
        "replays",
        "stops",
        "starts",
        "settles",
        "means",
        "says",
        "states",
        "names",
        "carries",
        "holds",
        "treats",
        "trusts",
        "lives",
        "works",
        "works",
        "here",
        "there",
        "now",
        "still",
        "again",
        "once",
        "twice",
        "first",
        "last",
        "next",
        "before",
        "after",
        "under",
        "over",
        "without",
        "within",
        "outside",
        "inside",
        "about",
        "above",
        "below",
        "between",
        "through",
        "against",
        "version",
    ];
    let commands: Vec<String> = subcommands
        .into_iter()
        .filter(|w| !prose.contains(&w.as_str()))
        .collect();
    let bin = common::catalyst_bin();
    let mut rejected = Vec::new();
    for sub in &commands {
        let run = common::run_with(
            &bin,
            &[sub, "--help"],
            &common::root(),
            false,
            &[],
            None,
            std::time::Duration::from_secs(20),
        );
        if run.status != Some(0) {
            rejected.push(format!("{sub} ({:?})", run.status));
        }
    }
    assert!(
        rejected.is_empty(),
        "C7: README.md shows commands the binary does not accept: {rejected:?}"
    );
}

#[test]
fn c7_every_cargo_command_the_readme_shows_exists() {
    let text = readme();
    let mut subs = std::collections::BTreeSet::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find("cargo ") {
        let at = start + pos;
        start = at + 6;
        let word: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '-')
            .collect();
        if !word.is_empty() {
            subs.insert(word);
        }
    }
    let cargo = common::which("cargo").unwrap_or_else(|| std::path::PathBuf::from("cargo"));
    // `--color never`: a `CARGO_TERM_COLOR=always` in the environment (CI sets
    // it) would otherwise wrap every command name in ANSI escapes.
    let list = common::run_with(
        &cargo,
        &["--color", "never", "--list"],
        &common::root(),
        false,
        &[],
        None,
        std::time::Duration::from_secs(30),
    );
    let known: Vec<String> = list
        .stdout
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    let unknown: Vec<&String> = subs.iter().filter(|s| !known.contains(s)).collect();
    assert!(
        unknown.is_empty(),
        "C7: README.md shows cargo commands this cargo does not know: {unknown:?}"
    );
}
