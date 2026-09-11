# Catalyst — Claude Code entrypoint

Read `AGENTS.md` first: it is the operating contract for any agent, every
command in it is accepted by the binary as written, and the skill at
`.claude/skills/using-catalyst/SKILL.md` gives step-by-step recipes for the
common jobs. `docs/interface.md` is the full contract.

Three things that matter most:

1. Every `catalyst` command prints one JSON object. `"ok":false` with a
   `code` and a `remedy` means nothing was written; follow the remedy.
2. You may propose and measure; you may not define success. Requests that set
   acceptance, change a tolerance or approve their own result are refused as
   `catalyst.authority_refused`. Do not route around that by editing files.
3. Build and test offline: `cargo build --offline --locked --release`,
   `cargo test --offline --locked`. No dependencies, ever.

`catalyst ai propose` runs the `claude` CLI you are already signed into, with
the prompt on stdin and no credential of Catalyst's own; `catalyst ai status`
shows the exact arguments before anything runs. On another plan, set
`CATALYST_AI_COMMAND`, `CATALYST_AI_MODEL` and `CATALYST_AI_ARGS`.

When you use the front end, ask it first: `catalyst tui --describe` returns
the panes, steps and keys as JSON, running nothing. Drive it with
`--headless --keys FILE --transcript FILE`; the interactive form sizes itself
to the person's terminal.
