# Catalyst for coding agents

Catalyst is a command-line verification system for engineering computations.
It measures a computation, differentiates it, checks every derivative
independently, records where it came from, and exports what it can prove.
This file is the operating contract for an AI agent using Catalyst as a tool
or working on this repository. It is written to be followed literally; every
command in it is accepted by the binary as written, and the acceptance suite
holds it to that.

The full interface is `docs/interface.md`. This file is the short path.

## The one rule of every command

Every `catalyst` command prints **one JSON object** on stdout.

- Success: `{"ok":true, …}`, exit 0.
- Refusal: `{"ok":false,"code":"catalyst.…","detail":"…","remedy":"…"}`,
  exit 2, and **nothing was written**. Read `code`, then do what `remedy`
  says. Do not retry the same command unchanged; do not parse `detail` for
  control flow, `code` is the stable string.
- `--help` on any command prints plain text and exits 0.
- Exit 101 (a panic) does not happen. If you ever see it, that is a bug: report
  the exact command and input.

Paths you name (`--out`, `--problem`, `--module`, `--artifact`, `--dir`, …)
must be relative, inside the current directory, with no `..` and no symlink.
Output directories must be empty or absent. Anything else is
`catalyst.path_refused` before any work starts.

## Build and test

```sh
cargo build --offline --locked --release
cargo test --offline --locked
```

No network is needed for either. The crate has no dependencies, and adding
one is not a change you may make. The binary is `target/release/catalyst`.

## The commands you will actually use

### Evaluate a problem

```sh
catalyst eval --problem spring.json
```

A `catalyst.problem.v1` document is `schema`, `name`, `goal`, `function`
(`func name(a, b) = expression`), `inputs` (every parameter), `domains`
(every parameter: `min`, `max`, `unit`). The answer is
`{"ok":true,"name","value","gradient":{…},"finite","cost"}`. A
`catalyst.problem.v2` document names a `computation` instead of a
`function`: `{"kind":"catalyst-expression","function":"…"}` or
`{"kind":"llvm","module":"file.ll","function":"name","fixed":{…}}`, and the
answer adds an `assurance` block that says which lane produced the number.

### Export, and run the export without Catalyst

```sh
catalyst export go --problem spring.json --out export
catalyst export r  --problem spring.json --out export-r
```

`export go` writes a Go module (standard library only) with fixtures and a
validation report; `cd export && go build -o exp . && ./exp fixtures.json`
runs it. `export r` writes base R. The fixtures carry the engine's own
numbers, so the export is checked against a measurement, not trusted.
`go/catalyst/` and `R/catalyst.R` in this repository re-check an export
independently and report how many cases they checked.

### Differentiate a compiled program (the Catalyst Gradient Compiler)

Compile with the compiler the user already has, then hand Catalyst the IR:

```sh
rustc --emit=llvm-ir -O -C panic=abort -C debuginfo=0 -C no-vectorize-slp -C no-vectorize-loops heat.rs -o heat.ll
catalyst differentiate llvm --module heat.ll --function heat --inputs x,y --at 1.5,2 --out derivative
catalyst differentiate run --artifact derivative --at 0.4,0.3
catalyst differentiate --describe
```

`--inputs` names the `double` parameters to differentiate with respect to;
`--at` gives a value for **every** parameter in declaration order. The
gradient is reported only after Catalyst's verifier confirmed it against
finite differences; a derivative it cannot confirm is
`catalyst.derivative_disagrees` and nothing is written. Read
`derivative/derivative.json` for the digest chain and the `assurance` block.
An unsupported instruction is `catalyst.llvm_unsupported` with the line
number; memory, arrays and vectors are outside this phase.
`catalyst.unknown_name` lists the parameter names the IR actually carries:
rustc leaves a parameter unnamed (`0`, `1`, …) when the body only copies it
into a `mut` local, so name it by that number in `--inputs`. Add `--source
model.rs` so the artifact records the source digest. A `--rules` entry
applies only to a call that survives the user's optimiser: mark the callee
`#[inline(never)]` (C: `__attribute__((noinline))`) or the rule is never
reached and `rules_applied` stays empty. `differentiate run` re-checks
`module.ll`, the digests in `derivative.json` and the listings `primal.cir`,
`derivative.cir` and `fixtures.json`; an edited file is
`catalyst.artifact_tampered`. An LLVM-lane artifact has
`assurance.portable_export: false`: `export go`/`export r` take a problem
file, not an artifact. Never run the user's compiled program yourself to
"check" a number: Catalyst already did, inside its own interpreter.

### Ask through the tool surface

```sh
catalyst tools discover
catalyst tools call --request request.json
```

`discover` lists the five tools with their request and response schemas:
`discover`, `validate_problem`, `evaluate`, `export_go`, `differentiate_llvm`.
A request is `{"schema":"catalyst.tool-request.v1","tool":"…","arguments":{…}}`
(`--request -` reads it from stdin). Use `validate_problem` before `evaluate`
when you wrote the document yourself.

**Authority boundary.** A request naming a tool `set_acceptance`,
`set_tolerance`, `approve_result` or `approve`, or carrying a top-level
`acceptance`, `tolerance` or `approve_own_result` argument, is
`catalyst.authority_refused`. You may propose a problem and ask for a
measurement; you may not define success, change a tolerance, or approve your
own result. Do not try to work around this by editing files: the tolerances
are the verifier's, not an input.

### Drive the terminal front end

```sh
catalyst tui --describe
catalyst tui --headless --keys keys.txt --transcript frames.txt --out export
```

`--describe` prints the panes, the five steps in order and each step's keys,
running nothing. On a real terminal `catalyst tui` sizes its frame to the
window (`stty size`); `--width N --height N` are for another size or for
headless runs, which default to 80 by 24. Too small a window is
`catalyst.tui_size_refused`, never a scrolled frame. A key script is one event per line: `text:<characters>`,
`enter`, `tab`, `backtab`, `up`, `down`, `backspace`, `esc`, `ctrl-s`,
`ctrl-c`. The flow is Goal → Inputs → Run → Results → Export. Read the
transcript's frame headers `--- frame N (step: …, state: …) ---`; a
measurement exists only once `state: measured`. The states are exactly
`proposed`, `running`, `measured`, `failed`, `inconclusive`. A validation
problem renders `Problem: …` and `Next step: …`; do what the next step says.
The headless transcript has no colour and no escape sequences, so parse it
as plain text.

### Catalyst's own AI, on the user's subscription

```sh
catalyst ai status
catalyst ai propose --goal "settle fast without overshoot" --out proposal.json --state ai-state.json
```

Catalyst holds no credential. It runs a CLI the user has already signed into
(`claude` by default; `--command`, `--model`, `--provider-args` or
`CATALYST_AI_COMMAND`, `CATALYST_AI_MODEL`, `CATALYST_AI_ARGS` select another
plan's CLI and argument shape). `ai status` prints the exact argument list
before anything runs. A proposal is validated as a problem before it is
written. Manual commands never consult the AI.

### Rehearse a failing local service

```sh
catalyst rehearse standin --out standin
catalyst rehearse init --dir rehearsal --target http://127.0.0.1:8080
catalyst rehearse status --dir rehearsal
```

Targets are local only (`unix:<path>`, `127.0.0.1`, `localhost`, `[::1]`);
anything else is refused before a request is made. `catalyst rehearse --help`
lists `replay`, `reduce`, `compare` and `report`.

## Refusal codes you will meet, and what to do

| code | do this |
| --- | --- |
| `catalyst.path_refused` | use a relative path inside the current directory |
| `catalyst.path_not_empty` | choose a new `--out` directory |
| `catalyst.syntax` | fix the JSON or the expression at the byte offset given |
| `catalyst.unknown_name` | every input and domain must be a parameter of the function |
| `catalyst.problem_incomplete` | add the missing `inputs` or `domains` entry (or `fixed` for an integer parameter) |
| `catalyst.input_out_of_domain` | move the value inside its declared range |
| `catalyst.authority_refused` | you asked to redefine success; do not |
| `catalyst.llvm_unsupported` | the instruction at that line is outside the subset; simplify the program or hold that part fixed |
| `catalyst.opaque_call` | the module calls something it does not define; define it or supply a `--rules` file |
| `catalyst.derivative_disagrees` | the derivative failed verification; report both numbers, do not widen anything |
| `catalyst.artifact_tampered` | a file in the artifact changed; regenerate it |
| `catalyst.ai_unavailable` | the AI command is not on `PATH`; say so, manual commands still work |
| `catalyst.computation_kind_unsupported` | compile to LLVM IR text and use kind `llvm` |
| `catalyst.portable_export_unavailable` | an LLVM derivative has no Go or R export |

## Working on the repository

- Source is `src/`; `src/gradient_compiler/` is the Catalyst Gradient
  Compiler; `src/tui/` the front end; `src/rehearse/` rehearsal;
  `src/adapter/` the local-adapter boundary; `docs/interface.md` the
  contract; `tests/acceptance_*.rs` the acceptance oracles that rule on it.
- `#![forbid(unsafe_code)]` stays. `[dependencies]` stays empty. No shell is
  ever spawned; the only external programs are `stty` (interactive TUI) and
  the configured AI command.
- Nothing Catalyst writes may contain a timestamp, an absolute path, the home
  directory or an environment value; exports are byte-deterministic.
- Finish every change with `cargo fmt --all`,
  `cargo clippy --offline --locked --all-targets -- -D warnings` and
  `cargo test --offline --locked --no-fail-fast`, all clean.
- A failing acceptance test means the code is wrong, not the test. Do not
  edit `tests/` to make something pass; report it.
- Report what you measured, with the command and its output. Never claim a
  pass you did not observe.
