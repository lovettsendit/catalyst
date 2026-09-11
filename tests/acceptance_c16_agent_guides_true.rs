//! C16 — the agent-facing guides are true. `AGENTS.md`, `CLAUDE.md` and
//! `.claude/skills/using-catalyst/SKILL.md` are what an AI reads before it
//! drives Catalyst, so a command shown there that the binary does not accept,
//! or a refusal code named there that the source does not define, is an error
//! the agent will make. The check is the one C7 applies to the README, over
//! all three files, plus the codes.
mod acceptance_common;
use acceptance_common as common;

const GUIDES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".claude/skills/using-catalyst/SKILL.md",
];

/// Every `catalyst <sub>` and `catalyst <sub> <sub2>` shown in a guide.
fn commands_in(text: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('`');
        let Some(rest) = line.strip_prefix("catalyst ") else {
            continue;
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        let Some(first) = words.first() else {
            continue;
        };
        if first.starts_with('-') && *first != "--help" {
            continue;
        }
        let second = words
            .get(1)
            .filter(|w| {
                !w.starts_with('-')
                    && w.chars()
                        .all(|c| c.is_ascii_lowercase() || c == '-' || c == '_')
            })
            .map(|w| w.to_string());
        out.push((first.to_string(), second));
    }
    out
}

#[test]
fn c16_every_command_a_guide_shows_is_accepted_by_the_binary() {
    let dir = common::scratch("guides");
    let top = common::catalyst(&["--help"], &dir, &[]);
    assert_eq!(top.status, Some(0));
    for guide in GUIDES {
        let text = common::read(guide).unwrap_or_else(|| panic!("C16: {guide} exists"));
        let commands = commands_in(&text);
        assert!(
            !commands.is_empty(),
            "C16: {guide} shows at least one catalyst command"
        );
        for (sub, second) in commands {
            if sub == "--help" {
                continue;
            }
            assert!(
                top.stdout.contains(&format!("  {sub} ")),
                "C16: {guide} shows `catalyst {sub}`, which `catalyst --help` does not list"
            );
            let help = common::catalyst(&[&sub, "--help"], &dir, &[]);
            assert_eq!(help.status, Some(0), "C16: `catalyst {sub} --help` exits 0");
            if let Some(second) = second {
                // A bare word after the subcommand is a form (`export go`,
                // `differentiate llvm`, `rehearse standin`, `ai status`, `tools call`)
                // and must appear in that command's own usage text.
                assert!(
                    help.stdout.contains(&second),
                    "C16: {guide} shows `catalyst {sub} {second}`, which `catalyst {sub} --help` does not document"
                );
            }
        }
    }
}

#[test]
fn c16_every_flag_a_guide_shows_is_documented_by_that_command() {
    let dir = common::scratch("guide-flags");
    for guide in GUIDES {
        let text = common::read(guide).unwrap();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('`');
            let Some(rest) = line.strip_prefix("catalyst ") else {
                continue;
            };
            let words: Vec<&str> = rest.split_whitespace().collect();
            let Some(sub) = words.first() else { continue };
            if sub.starts_with('-') {
                continue;
            }
            let help = common::catalyst(&[sub, "--help"], &dir, &[]);
            for flag in words.iter().filter(|w| w.starts_with("--")) {
                let flag = flag.trim_end_matches('`').trim_end_matches(',');
                assert!(
                    help.stdout.contains(flag),
                    "C16: {guide} shows `catalyst {sub} … {flag}`, which `catalyst {sub} --help` does not document"
                );
            }
        }
    }
}

#[test]
fn c16_every_refusal_code_a_guide_names_exists_in_the_source() {
    let source: String = common::src_files().into_iter().map(|(_, t)| t).collect();
    for guide in GUIDES {
        let text = common::read(guide).unwrap();
        let mut start = 0;
        while let Some(pos) = text[start..].find("catalyst.") {
            let at = start + pos;
            let code: String = text[at..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '.' || *c == '_' || *c == '-')
                .collect();
            start = at + code.len().max(9);
            // Schemas (`catalyst.problem.v1`) and prose (`catalyst.` at a
            // sentence end) are not codes; a code is `catalyst.<word>`.
            // A schema name carries a version (`catalyst.derivative-rules.v1`)
            // or a hyphen; a refusal code is one lowercase word.
            if code.matches('.').count() != 1 || code.len() < 12 || code.contains('-') {
                continue;
            }
            let word = &code["catalyst.".len()..];
            if word.starts_with("problem")
                || word.starts_with("tool")
                || word.starts_with("tui")
                || word.starts_with("derivative-")
            {
                continue;
            }
            assert!(
                source.contains(&format!("\"{code}\"")),
                "C16: {guide} names refusal code `{code}`, which the source does not define"
            );
        }
    }
}

#[test]
fn c16_the_guides_state_the_authority_boundary_and_the_one_object_rule() {
    for guide in GUIDES {
        let text = common::read(guide).unwrap().to_ascii_lowercase();
        assert!(
            text.contains("authority_refused"),
            "C16: {guide} states the authority boundary"
        );
        assert!(
            text.contains("\"ok\":false")
                || text.contains("ok:false")
                || text.contains("`\"ok\":false`")
                || text.contains("ok\":false"),
            "C16: {guide} states the refusal shape"
        );
    }
    let skill = common::read(".claude/skills/using-catalyst/SKILL.md").unwrap();
    assert!(
        skill.starts_with("---\nname: using-catalyst\n"),
        "C16: the skill carries its frontmatter"
    );
    assert!(
        skill.contains("tui --describe") && skill.contains("--headless"),
        "C16: the skill tells an agent how to ask the front end"
    );
}

/// The codes the guide's table teaches an agent to handle are the codes an
/// agent can look up: every one of them appears in `tools discover`'s
/// `errors` array with a meaning and a remedy, so the guide and the surface
/// cannot drift apart.
#[test]
fn c16_every_code_in_the_guides_table_is_in_discover() {
    let dir = common::scratch("guide-discover");
    let discover = common::assert_ok(&common::catalyst(&["tools", "discover"], &dir, &[]));
    let listed: Vec<String> = discover
        .get("errors")
        .and_then(common::Json::as_arr)
        .expect("C16: errors")
        .iter()
        .map(|e| e.str_field("code").to_owned())
        .collect();
    let text = common::read("AGENTS.md").unwrap();
    let mut missing = Vec::new();
    for line in text.lines() {
        // The table rows: `| `catalyst.code` | do this |`.
        let Some(rest) = line.strip_prefix("| `catalyst.") else {
            continue;
        };
        let code = format!("catalyst.{}", rest.split('`').next().unwrap_or(""));
        if !listed.contains(&code) {
            missing.push(code);
        }
    }
    assert!(
        missing.is_empty(),
        "C16: AGENTS.md teaches these codes, and `tools discover` does not list them: {missing:?}"
    );
}
