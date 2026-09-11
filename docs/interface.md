# Catalyst interface reference

This is the contract Catalyst is built and tested against. The acceptance
tests in `tests/acceptance_*.rs` hold the binary to every statement here;
where this text and a test disagree, the test rules and this text is
corrected. Section numbers are stable and are referred to from the README,
the agent guides and the tests. Nothing here needs a dependency.

Sections: 0 conventions · 1 problems · 2 `eval` · 3 `export go` · 4 the
guided flow · 4b the panelled layout · 5 tools and Catalyst's own AI · 6 the
local-adapter boundary · 7 the label · 8 failure rehearsal · 9 the R export ·
10 one request on stdin · 11 Go in the repository · 12 the Catalyst Gradient
Compiler.

## 0. Conventions

- The binary is `catalyst` (`src/main.rs`). Every subcommand prints one JSON
  object to stdout. Success objects carry `"ok":true`. A refusal is
  `{"ok":false,"code":"catalyst.…","detail":"…","remedy":"…"}` on stdout with
  exit code 2. Exit code 101 (a panic) never happens on any input. `--help` on
  any command prints plain usage text and exits 0.
- **Paths a user names** — every value of `--out`, `--dir`, `--transcript`,
  `--save`, `--state`, `--trace`, `--patterns`, `--result`, `--scenario`,
  `--held-out`, `--compare`, `--keys`, `--request`, `--context`, `--problem`,
  `--resume`, `--module`, `--rules`, `--source` and `--artifact` (the list
  `PATH_FLAGS` in `src/cli.rs`), and every socket path: must contain no `..`
  component, contain no symbolic link, and resolve inside the current
  directory. A relative path is the normal case; an absolute path is accepted
  only if it lies inside the current directory. Otherwise
  `catalyst.path_refused`, and nothing is written. The rule runs before the
  command does anything, on every command, including one that then refuses
  for another reason. Output directories are created if absent and must be
  empty if present (`catalyst.path_not_empty`).
- **Numbers in JSON.** Every number is written in the shortest form that reads
  back as the same double, and a whole number carries a fractional part
  (`"version":1.0`, `"points":5.0`), so a reader always sees a JSON number of
  one kind. The examples in this document write `1` where the binary writes
  `1.0`, and omit the `"ok":true` member that every success object carries.
- Nothing Catalyst writes contains a timestamp, an absolute path, the home
  directory, or any environment variable value. Exports are byte-for-byte
  deterministic for the same inputs.
- No user-supplied string is ever passed to a shell; `sh`, `bash`, `cmd` are
  never spawned. The only external programs Catalyst may run are `stty`
  (interactive TUI only) and the configured AI command (AI only).
- Validation messages always say what to do: JSON refusals carry `remedy`; the
  TUI renders `Problem: …` followed by `Next step: …`.

## 1. Problems (`catalyst.problem.v1`, and `catalyst.problem.v2`)

A v1 document names a `function` in the expression language, as below. A v2
document names a `computation` instead — the same expression, or a compiled
program handed to the Catalyst Gradient Compiler — and is specified in §12.8.
Everything in this section applies to both.

```json
{
  "schema": "catalyst.problem.v1",
  "name": "spring",
  "goal": "settle within 0.4 s with less than 5 % overshoot",
  "function": "func spring(k, c) = 8 / c + 100 * exp(0 - 3.141592653589793 * c / sqrt(4 * k - c * c))",
  "inputs": { "k": 12.0, "c": 1.5 },
  "domains": {
    "k": { "min": 1.0, "max": 100.0, "unit": "N/m" },
    "c": { "min": 0.1, "max": 6.0, "unit": "N*s/m" }
  }
}
```

- `function` is the existing expression language (`func name(a, b) = …`).
  Limits: at most 65 536 bytes (`catalyst.function_too_long`), nesting at most
  64 (the parser's own refusal).
- Every parameter of `function` needs an entry in `inputs` and in `domains`;
  a missing one is `catalyst.problem_incomplete`. An input or domain name that
  is not a parameter is `catalyst.unknown_name`. A name used in the function
  body that is not a parameter is the parser's `catalyst.unknown_name`.
- Numbers must be finite (`1e999`, `NaN` refuse as `catalyst.syntax`). `min <
  max`; each input lies within its domain (`catalyst.input_out_of_domain`).
  `unit` is any string, possibly empty. `name` is `[a-z][a-z0-9_-]{0,31}`.
- Unknown top-level keys are ignored and are **never** forwarded anywhere
  (see §5, only approved data leaves the process).
- Invalid JSON, an empty file, wrong types: `catalyst.syntax`.

## 2. `catalyst eval --problem FILE`

`{"ok":true,"name":"spring","value":…,"gradient":{"k":…,"c":…},"finite":true,"cost":{"primal_insts":…,"adjoint_insts":…}}`.
Non-finite numbers are written as JSON `null` and `finite` is `false`.

## 3. `catalyst export go --problem FILE --out DIR`

Writes exactly these files into `DIR`, then prints
`{"ok":true,"out":"DIR","files":[…],"cases":N}`:

| File | Content |
| --- | --- |
| `go.mod` | `module catalyst_export_<name>` and a `go 1.21` directive; **no** `require` |
| `function.go` | `package main`. `var InputNames = []string{…}` in declaration order; `var InputMin`, `var InputMax []float64`; `func Value(in []float64) float64`; `func Gradient(in []float64) (float64, []float64)` (value, partials in `InputNames` order); `func InDomain(in []float64) (bool, string)`. Generated from the engine's optimised IR and its reverse-mode adjoint as straight-line Go; imports only `math` |
| `main.go` | `package main`; `main` reads the fixtures file named by `os.Args[1]` and prints one JSON array; per case: `{"case":"…","value":v,"gradient":{"k":…}}`, or `{"case":"…","refused":"out of domain: …"}` when `InDomain` is false, or `{"case":"…","nonfinite":true}` when the value or a partial is not finite. Imports only standard library, and never `unsafe`, `C`, `os/exec`, `net`, `syscall` |
| `parameters.json` | `{"schema":"catalyst.export-parameters.v1","name","goal","function","inputs","domains","units":{name:unit},"limitations":[…]}` |
| `fixtures.json` | `{"schema":"catalyst.fixtures.v1","tolerance":{"relative":1e-9,"absolute":1e-12},"cases":[…]}` |
| `validation-report.md` | Plain text: the problem, the tolerance, every case with the Rust engine's value and partials, how to build and run the Go, and both limitation sentences verbatim |

Fixture cases (`kind` is `normal`, `boundary` or `out_of_domain`):

- at least three `normal` cases: the problem's own inputs, and two interior
  points (for every parameter, min + 0.25·(max−min) and min + 0.75·(max−min));
- for **every** parameter `p`: `boundary-p-min` and `boundary-p-max` (that
  parameter at its bound, the others at the problem's inputs);
- at least one `out_of_domain` case (a parameter below its min) with
  `"expected":{"refused":true}`.

`expected` for a normal or boundary case is `{"value":…,"gradient":{…}}`
computed by the Rust engine, or `{"nonfinite":true}` when the engine's value
or a partial is not finite there. A boundary case may carry
`"tolerance":{"relative":…,"absolute":…,"reason":"…"}` (wider only, with a
reason). The two limitation sentences, verbatim in `parameters.json`
`limitations` and in `validation-report.md`:

- `Arbitrary Go is not differentiable: only the exported function and its gradient are generated from the engine's IR.`
- `Floating-point results are not identical across languages or platforms: the fixtures declare a tolerance instead of exact equality.`

## 4. The guided terminal flow: `catalyst tui`

One state machine, three front ends:

- `catalyst tui` — interactive; needs a TTY; raw mode through `stty`; ANSI
  rendering to stdout.
- `catalyst tui --keys FILE [--out DIR]` — the same ANSI renderer, input from
  the key script instead of the keyboard; no TTY needed; `stty` not run.
- `catalyst tui --headless --keys FILE --transcript FILE` — the plain-text
  renderer; every frame appended to the transcript as
  `--- frame N (step: <step>, state: <state>) ---` followed by the frame.
- `--save FILE` (default `catalyst-tui-state.json`) is where `ctrl-s` writes
  `{"schema":"catalyst.tui-state.v1","step":"…","state":"…","function":"…","fields":{…}}`;
  `--resume FILE` restores the function, every field and the state; the
  run is a measurement, not a field, so a session saved at Results comes
  back at Run, one key from reproducing the numbers.

Key script: one event per line: `text:<characters>`, `enter`, `tab`,
`backtab`, `up`, `down`, `backspace`, `esc`, `ctrl-s`, `ctrl-c`. `ctrl-c`
quits at once. `esc` goes back one step and keeps every value. `ctrl-s` saves
and stays. End of script quits.

Steps, rendered in a header `Goal > Inputs > Run > Results > Export` with the
current step in square brackets (`[Inputs]`), and a line `state: <state>`:

1. **Goal** — one field, the function source. `enter` parses; on success
   → Inputs with state `proposed`.
2. **Inputs** — per parameter in declaration order four fields: value, min,
   max, unit; `tab`/`backtab` move between fields. `enter` validates all
   (finite numbers, min < max, value within [min, max]) → Run.
3. **Run** — a summary; `enter` renders a frame with `state: running` and a
   `progress:` line, evaluates, then → Results with state `measured` (value and
   every partial finite), `failed` (the value is not finite, or the engine
   refused) or `inconclusive` (value finite, a partial not finite).
4. **Results** — value and a per-parameter table of partials; `enter` → Export.
5. **Export** — one field, the output path (default `export/<name>`); `enter`
   writes the Go export of §3 and renders `exported: <path>`.

The five state labels `proposed`, `running`, `measured`, `failed`,
`inconclusive` are the only states; `catalyst tui --states` prints them as a
JSON array. A validation failure renders `Problem: <what is wrong>` and
`Next step: <what to do>`.

## 4b. The panelled layout

One terminal shows several at once. Every frame the guided flow renders, in
both the headless and the terminal front end, is a single closed rectangle
divided into four panes, drawn with box-drawing characters and readable with no
colour at all:

- `catalyst` — the step being worked: the flow header
  `Goal > Inputs > Run > Results > Export` with the current step in square
  brackets, the `state: <state>` line, and the step's own fields, results or
  messages.
- `measurements` — the value and every partial once a run has produced them,
  and nothing but a dash before that. It never shows a number that is not
  currently true.
- `steps` — the five steps in order, each marked `done`, `here` or `-`.
  Exactly one is `here`, and it is the step the frame header names.
- `keys` — what works on this step, naming at least `enter`, `ctrl-s` and
  `ctrl-c`, and never naming a key the step ignores.

Rules the layout keeps:

- Every line of a frame has the same display width. The first line opens with
  `┌` and closes with `┐`, the last with `└` and `┘`, and every line between is
  bounded on both sides.
- On a real terminal the frame is the size of that terminal, read once from
  `stty size` before the first frame, so nothing scrolls; headless, scripted
  and `--describe` runs are 80 columns by 24 rows, which fits any terminal.
  `--width N` and `--height N` set it either way, and either flag switches
  detection off. A size no layout can honour is refused as
  `catalyst.tui_size_refused` rather than drawn wrong.
- A value the operator typed is never split across a pane border: it appears
  whole, exactly as typed.

`catalyst tui --describe` prints the layout as data, so an agent can drive the
front end without reading a drawing, and exits 0 having run nothing and written
nothing:

```json
{
  "schema": "catalyst.tui-layout.v1",
  "width": 80, "height": 24,
  "panes": [{ "name": "catalyst", "row":1,"col":1,"width":47,"height":18 }],
  "steps": [{ "name": "goal", "keys": ["enter", "backspace", "ctrl-s", "ctrl-c"] }]
}
```

The panes are the four named above, the steps are `goal`, `inputs`, `run`,
`results`, `export` in that order, and each step names the keys that work on it.

## 5. The tool surface and Catalyst's own AI

`catalyst tools discover` prints
`{"schema":"catalyst.tools.v1","version":1,"label":"<the label of §7>","tools":[…],"errors":[…],"continuation":{"rules":[…],"resume":"catalyst ai propose --state FILE --resume"}}`.
Each tool has `name`, `description`, `request_schema`, `response_schema`.
Tools: `discover`, `validate_problem`, `evaluate`, `export_go`,
`differentiate_llvm` (§12.9). `discover` returns this same document, so the
adapter's `discover` capability of §6 translates into a request the surface
actually serves rather than one it would refuse; the capability list and the
tool list are one list. Each error
has `code`,
`meaning`, `remedy`.

`catalyst tools call --request FILE` (`-` for stdin). Request:
`{"schema":"catalyst.tool-request.v1","tool":"evaluate","arguments":{"problem":{…}}}`
(`export_go` also takes `"out"`). Response:
`{"schema":"catalyst.tool-response.v1","tool":"…","ok":true,"result":{…}}` or a
refusal. A malformed request (`catalyst.tool_request_invalid`, unknown tool
`catalyst.tool_unknown`) changes no state and writes no file. A request whose
tool is `set_acceptance`, `set_tolerance`, `approve_result` or `approve`, or
whose arguments carry a top-level `acceptance`, `tolerance` or
`approve_own_result` key, is refused as `catalyst.authority_refused`: a model
cannot redefine success, weaken a check, or approve its own result.

`catalyst ai status [--command NAME] [--model NAME] [--provider-args TEMPLATE]` prints
`{"ok":true,"available":<bool>,"command":"claude","model":"claude-opus-5","arguments":["-p","--model","claude-opus-5","--output-format","json"],"detail":"…","remedy":"…"}`
(`arguments` is the exact argument list the command would be run with;
`remedy` names the next step when unavailable) and exits 0. Manual commands
never consult the AI command.

**Catalyst holds no credential and makes no provider network call.** It has no
API-key setting, no HTTP client for any provider, and no account of its own.
It runs a command-line program that the user has already signed into with
whatever plan they pay for, and reads that program's answer. The only
environment values Catalyst reads at all are `CATALYST_AI_COMMAND`,
`CATALYST_AI_MODEL`, `CATALYST_AI_ARGS` and `PATH`; a credential in the
environment is never read, never placed in a request and never written to any
file Catalyst produces (`tests/acceptance_3_tool_surface.rs` holds this).

`catalyst ai propose --goal TEXT --out FILE --state FILE [--context PROBLEM] [--command PATH] [--model NAME] [--provider-args TEMPLATE] [--transcript FILE] [--resume]`:
runs the command (default `claude`, overridable by `--command` or the
environment variable `CATALYST_AI_COMMAND`; model default `claude-opus-5`,
overridable by `--model` or `CATALYST_AI_MODEL`) with the prompt on stdin and
no shell.

The argument list is `--provider-args`, else `CATALYST_AI_ARGS`, else the
default `-p --model {model} --output-format json`. A template is split on
whitespace and every occurrence of `{model}` is replaced with the resolved
model name; a template that does not mention `{model}` is run without one,
because some subscription CLIs take no model argument. This is what makes
Catalyst work with whatever plan its user has: a Claude Max seat through
`claude`, a Codex Pro seat through `codex` with its own argument shape, any
other signed-in CLI through its own. An empty template is refused as
`catalyst.ai_arguments_invalid`. The
prompt contains only approved data: the goal text, the discovery document,
the approved fields of the optional `--context` problem (`schema`, `name`,
`goal`, `function`, `inputs`, `domains`), and the instruction to answer with
one `catalyst.problem.v1` object. Nothing from the environment and no other
field of the context file (for example `private` or `scratch`) is ever placed
in it. The
command's stdout is read as the answer text: when it is a CLI envelope
(`{"type":"result","result":"<text>",…}`) the text is that `result`, and
otherwise the whole of stdout is the text, so a CLI that simply prints its
answer is accepted as it stands. The first JSON object in that text (fences
allowed) is validated as a problem (§1) and written to `--out`; output `{"ok":true,"out":…,"validated":true,"proposal":{…}}`.
An invalid proposal is `catalyst.proposal_refused`. If the command is absent
(`catalyst.ai_unavailable`), fails or is interrupted (`catalyst.ai_call_failed`),
the state file keeps `{"schema":"catalyst.ai-state.v1","goal":…,"pending":true,"attempts":[…]}`
and the remedy says to run again with `--resume`; without `--resume` while
pending: `catalyst.ai_pending`. `--transcript` records the exact prompt sent
and the raw stdout received.

## 6. The local-adapter boundary

`catalyst adapter --help`, `catalyst tools discover` and the conformance
report all carry the label of §7 verbatim. Adapter code lives under
`src/adapter/` (or `src/adapter.rs`) and spawns no process and opens no socket.

- `catalyst adapter conformance` prints
  `{"schema":"catalyst.adapter-conformance.v1","label":"…","inference":false,"fixtures":[{"name":"…","ok":true,"detail":"…"},…],"passed":N,"failed":0}`
  and exits 0. Fixture names include `request-valid`,
  `request-malformed-refused`, `response-valid`,
  `response-malformed-refused`, `discovery`, `error-mapping`,
  `continuation`. They are built in and need no inference.
- `catalyst adapter translate --request FILE` turns
  `{"schema":"catalyst.adapter-request.v1","capability":"evaluate","arguments":{…}}`
  into a validated `catalyst.tool-request.v1` object (printed with
  `"ok":true,"request":{…}`); an unsupported capability is
  `catalyst.adapter.capability_unsupported` with a remedy that lists the
  supported ones (`discover`, `validate_problem`, `evaluate`, `export_go`,
  `differentiate_llvm`).

## 7. The label

`local-adapter extension interface; live local-model compatibility untested`

## 8. Failure rehearsal: `catalyst rehearse …`

Targets: `unix:<relative socket path>`, `http://127.0.0.1:PORT`,
`http://localhost:PORT`, `http://[::1]:PORT`. Anything else — another host, an
IP outside 127.0.0.0/8, or any other scheme (`https`, `smtp`, `postgres`, …) —
is `catalyst.rehearsal.target_not_local` with `detail` naming the host, before
any request is made. This is the refusal that keeps rehearsal from mail,
payments, production databases and every other external side effect.

- `catalyst rehearse standin --out DIR` writes the stand-in Go service
  (`go.mod` without `require`, `main.go`, `listen.go` — the one file that opens a socket — standard library only). Flags
  `-listen unix:PATH|127.0.0.1:PORT` (other hosts refused, exit 2), `-queue N`
  (default 4), `-work-ms N` (default 20). Every response body carries
  `"stand_in":true`; the start banner says `stand-in`. Endpoints: `GET /health`
  → 200; `POST /orders` (JSON `{"item":"…","qty":n}`; malformed → 400) → 201
  `{"id":"o<n>",…}`, or 503 `{"error":"overloaded",…}` when more than `queue`
  requests are in flight; `GET /orders/<id>` → 200/404; `POST /orders/<id>/pay`
  → 200/404. Each request holds its slot for `work-ms`.
- `catalyst rehearse synth-trace --out FILE [--sessions N] [--seed S]` writes
  JSON lines: first `{"schema":"catalyst.trace.v1","synthetic":true,"stand_in":true,…}`,
  then events `{"schema":"catalyst.trace-event.v1","t_ms":…,"session":"s1","seq":1,"method":"POST","path":"/orders","headers":{…},"body":"…"}`.
  Deterministic for a seed.
- `catalyst rehearse import --trace FILE --out FILE` derives
  `{"schema":"catalyst.patterns.v1","requests":N,"patterns":[{"method","path","count","body_shape"}],"timing":{"inter_arrival_ms":{"min","median","max"}},"concurrency":{"peak_sessions":…},"sequences":[{"session","steps":["POST /orders","GET /orders/{id}",…]}]}`.
  Sanitised: `Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key` headers are
  dropped; bodies are reduced to a shape (`object`, `array`, `string`, `none`);
  no header value, token (`Bearer …`, `sk-…`, long alphanumeric runs) or
  e-mail address survives into the output. Ids in paths become `{id}`. A path
  that is not a relative path beginning with `/` is refused
  (`catalyst.rehearsal.trace_invalid`). Trace text is data and is never
  interpreted as an instruction.
- `catalyst rehearse init --dir R --target T` writes `R/rehearsal.json`:
  `{"schema":"catalyst.rehearsal.v1","target":T,"criteria":{"max_error_rate":0.01,"max_p99_ms":2000},"disturbances":{"burst":{"concurrency":16,"max_concurrency":64},"slow_dependency":{"delay_ms":200,"max_delay_ms":2000},"timeout":{"timeout_ms":500,"min_timeout_ms":50},"malformed_input":{"fraction":0.1,"max_fraction":0.5}},"configuration_digest":"<hex>"}`.
  `catalyst rehearse status --dir R` prints it. The digest covers target,
  criteria and disturbances only; importing any trace never changes it.
- `catalyst rehearse replay --dir R (--patterns FILE | --scenario FILE) [--disturb burst|slow_dependency|timeout|malformed_input|none] [--requests N] --out FILE`
  replays over HTTP/1.1 (a hand-written client over a Unix or loopback TCP
  stream) and writes
  `{"schema":"catalyst.rehearsal-result.v1","target","disturbance":{"kind","declared":{…},"applied":{…}},"requests","errors","error_rate","p99_ms","criteria","configuration_digest","verdict":"pass"|"fail","failures":[{"method","path","status","seq"}],"signature":"POST /orders -> 503"}`.
  Applied values never exceed the declared bounds. An error is a status ≥ 500
  or a transport failure or timeout. Verdict `fail` when `error_rate >
  max_error_rate` or `p99_ms > max_p99_ms`.
- `catalyst rehearse reduce --result FILE --dir R --out FILE` writes the
  minimal scenario `{"schema":"catalyst.scenario.v1","signature","concurrency","steps":[{"method","path","body"}],"disturbance":{"kind","applied":{…}},"expected":{"status":503}}`
  and a copy under `R/regressions/`. Replaying it against the same target
  reproduces the same `signature`.

  The scenario records **the disturbance that produced the failure**, and
  `replay --scenario` applies it, because for two of the four disturbances the
  failure is a property of the disturbance rather than of the traffic. A
  `timeout` or `slow_dependency` failure replayed without its disturbance does
  not reproduce: the slow target simply answers slowly and passes. `kind` is
  `none` when the failure needed no disturbance, and then nothing is applied.
- `catalyst rehearse held-out --out FILE [--seed S]` writes
  `{"schema":"catalyst.scenarios.v1","scenarios":[…]}`.
- `catalyst rehearse compare --dir R --baseline T1 --candidate T2 --scenario FILE --held-out FILE --out FILE`
  replays the regression scenario and every held-out scenario against both
  targets under the unchanged criteria and writes
  `{"schema":"catalyst.rehearsal-compare.v1","configuration_digest","baseline":{"regression":{…},"held_out":[…]},"candidate":{…},"improvement":{"regression_fixed":bool,"held_out_passed_before":n,"held_out_passed_after":n}}`.
- `catalyst rehearse report --compare FILE --out FILE` writes Markdown with
  the sections `## Measured improvement`, `## Remaining failures`,
  `## Remaining uncertainty`, and never the words `failure-free`, `guarantee`,
  `guaranteed`, `will never fail`, `will not fail`, `cannot fail`.

## 9. The R export and Catalyst's R side

Catalyst's promise is a checked, self-contained export. Go was the first
language it could be handed to; R is the second, for the people who do this
work in R.

`catalyst export r --problem FILE --out DIR` obeys the same path rule and the
same problem rules as `catalyst export go`, and prints
`{"ok":true,"out":DIR,"files":[…],"cases":N}`. A refused export writes nothing.
It writes five files, all base R — no package is loaded, by any spelling, so a
reader who is handed the directory can run it with the R they already have:

- `function.R` — `catalyst_value(p)` and `catalyst_gradient(p)`, where `p` is a
  named list of parameter values. Generated from the same intermediate form as
  the Go export, so the arithmetic is the same arithmetic.
- `parameters.R` — `catalyst_parameters`, one entry per parameter in
  declaration order, each with `value`, `min`, `max` and `unit`.
- `fixtures.R` — `catalyst_fixtures`, the same cases as the Go export's
  `fixtures.json`, carrying the same expected value and gradient.
- `test_function.R` — zero-argument functions named `test_*` that check every
  fixture within the declared tolerance and stop, naming the case, on a
  disagreement. It runs under `Rscript --vanilla`.
- `validation-report.md` — the same honest report the Go export carries,
  including a `Remaining uncertainty` section, and never the words
  `failure-free`, `guarantee`, `guaranteed`, `will never fail`,
  `will not fail`, `cannot fail` or `proven correct`.

The R and Go exports of one problem agree: every expected value in
`fixtures.json` appears in `fixtures.R`, and R reproduces it.

Catalyst's own R side lives in `R/`, and the host's R lane checks it:

- `R/catalyst.R` — base-R helpers. `catalyst_check_export(dir)` sources an
  export directory, runs its fixtures and returns the number of cases checked;
  it stops, naming the case, when one disagrees. `catalyst_close(a, b, rel,
  abs)` is the tolerance rule, the same rule the Rust engine uses.
- `R/test_catalyst.R` — zero-argument `test_*` functions covering those
  helpers, including that a corrupted fixture is refused rather than passed.

## 10. One request on stdin, for a caller that cannot link a crate

Not every caller is a Rust crate or a shell script that wants subcommands. A
notebook, a training loop in another language, or an agent that can run a
command but cannot link one needs a single request and a single answer. That is
what `catalyst::api::solve` is, and `examples/grad.rs` is the whole program
that exposes it:

```sh
echo '{"source": "func f(x, y) = x*y + sin(x)", "at": [0.7, 1.3]}' \
  | cargo run --quiet --example grad
```

```json
{"ok":true,"value":1.554217687237691,
 "gradient":{"x":2.0648421872844884,"y":0.7},
 "cost":{"primal_insts":5,"adjoint_insts":13}}
```

- The request is **one JSON object carrying exactly `source` and `at`**, with
  finite numeric coordinates, at most 1 MiB of UTF-8. A duplicate field, an
  extra field, a missing one or a non-finite coordinate is refused rather than
  partly interpreted.
- The answer is one JSON object on stdout. A refusal has the same `ok` field
  and a stable `code`, so a caller parses one shape and branches on one field.
- A value that is not finite is written as `null`, because JSON carries no
  infinity and no NaN, and a document that cannot be parsed is worse than one
  that says there is no number here.
- Nothing is written to stderr, and a well-formed run always exits 0. A caller
  reading stdout never has to interpret a signal to find out what happened.
- One request per invocation, read to end of input.

## 11. Go that lives in the repository

Catalyst emits Go and rehearses Go, so a reader who opens the repository
expects to find Go in it. Two pieces are not generated per problem, so both are
checked in and checked by the Go toolchain where they live rather than only
after Catalyst has written them somewhere temporary.

### `go/standin/`

The rehearsal stand-in service, as files: `go.mod` with no `require`,
`main.go` and `listen.go` (the one file that opens a socket), standard library
only. Catalyst embeds them rather than restating
them, so what `catalyst rehearse standin --out DIR` writes is byte-identical to
what the repository holds, by construction rather than by agreement. Its
behaviour is unchanged and is specified in §8: it listens on a Unix socket or
on loopback and refuses anywhere else, every response body carries
`"stand_in":true`, and the start banner says `stand-in`.

### `go/catalyst/`

The sibling of `R/catalyst.R`: a package that takes an export directory and
checks it, so someone handed an export can verify it with the Go they already
have. Standard library only, `go.mod` with no `require`.

```sh
cd go/catalyst
go run . -export ../../export/spring
```

- It reads the export's `fixtures.json`, evaluates the export's own function
  over every case, and compares within the tolerance the export declares.
- It prints **how many cases it checked** and exits 0. A count rather than a
  bare pass, because "it passed" and "it checked nothing" must not look the
  same. `R/catalyst.R` reports a count for the same reason.
- A case that disagrees stops it with a non-zero exit and a message naming the
  case.
- A `_test.go` beside it covers both directions, including that a corrupted
  fixture is refused rather than passed.

Neither piece imports anything outside the standard library. The ban on
`unsafe`, `C`, `os/exec` and `syscall` that applies to an export applies to
them too; `net` is allowed in exactly one file of the stand-in, `listen.go`,
because a service that listens has no other way to.

## 12. The Catalyst Gradient Compiler

Catalyst's engine differentiates SSA programs: blocks, branches, phis, integer
counters, comparisons and selects, not only straight-line expressions. Until
now the only way into it was the expression language of §1. The **Catalyst
Gradient Compiler (CGC)** is a second front door: it takes the LLVM IR a real
compiler emits for a real program, lowers it to the engine's own IR, produces a
derivative, and hands that derivative back to Catalyst as an artifact that
Catalyst validates independently. CGC proposes; Catalyst's verifier decides.

Two lanes, one engine, one artifact, one verifier:

```text
   expression (§1)                      program (Rust, C, C++, …)
        │                                        │
   Catalyst parser                        the user's own compiler
        │                                        │
        ▼                                        ▼
   Catalyst IR                     LLVM IR text (`.ll`)  ──►  CGC front end
        │                                                          │
        └──────────────────►  Catalyst IR  ◄───────────────────────┘
                                   │
                        optimise, analyse, differentiate
                                   │
                                   ▼
                   derivative artifact  (`catalyst.derivative.v1`)
                                   │
                    Catalyst's finite-difference verifier
                                   │
                              pass / refuse
```

CGC is a subsystem of Catalyst with its own name and its own directory,
`src/gradient_compiler/`. Its identity is its own: no file in this tree
presents Catalyst, or any part of it, as another project under a new name.
Compiler-level automatic differentiation is a general technique with published
prior art, and prior art may be acknowledged as prior art, in one sentence,
and that is all. `tests/acceptance_10_gradient_compiler.rs` holds the tree to this.

### 12.1 Backends

Differentiation is a backend behind one interface, so that the engine is a
controlled system for producing and checking derivatives rather than one
algorithm wired into everything:

```rust
pub trait DifferentiationBackend {
    fn differentiate(&self, program: &Program, request: &Request) -> Result<DerivativeArtifact, Refusal>;
}
```

(The exact type names other than the trait, its method and `DerivativeArtifact`
are the implementer's; the trait and the two names are bound.) Two backends
exist:

| name | input | modes |
| --- | --- | --- |
| `native` | `catalyst-ir`, from the expression language of §1 | `reverse` |
| `cgc` | `llvm-ir`, textual LLVM IR | `forward`, `reverse` |

`catalyst differentiate --describe` prints, having run nothing and written
nothing:

```json
{"schema":"catalyst.differentiation.v1",
 "backends":[{"name":"native","input":"catalyst-ir","modes":["reverse"]},
             {"name":"cgc","input":"llvm-ir","modes":["forward","reverse"]}],
 "artifact":"catalyst.derivative.v1",
 "validation":{"method":"central finite differences over the primal","relative_tolerance":1e-6,"absolute_tolerance":1e-9}}
```

The numerical verifier is **not** a backend: it is Catalyst's, and it never
produces a derivative, only a verdict on one.

### 12.2 `catalyst differentiate llvm`

```
catalyst differentiate llvm --module FILE.ll --function NAME --inputs a,b[,…]
    --at v1,…,vn --out DIR [--mode forward|reverse] [--rules FILE] [--source FILE]
```

- `--module` is a textual LLVM IR file (`.ll`): what `rustc --emit=llvm-ir`
  and `clang -S -emit-llvm` write. Catalyst never runs a compiler; the user
  runs theirs and hands Catalyst the result. Bitcode (`.bc`) is not read.
  At most 4 MiB (`catalyst.llvm_module_too_long`).
- `--function` names a `define` in the module (without the `@`). Absent:
  `catalyst.llvm_function_not_found`, and the detail lists the functions the
  module does define.
- `--inputs` names the parameters to differentiate with respect to, by their
  IR names without the `%` (`x`, `y`), in any order; the gradient is reported
  in the order given. Each must be a `double` parameter of the function
  (`catalyst.llvm_input_not_real` otherwise; a name that is not a parameter is
  `catalyst.unknown_name`). Parameters not named are held fixed.
- `--at` gives a value for **every** parameter of the function, in declaration
  order, comma separated. Wrong count: `catalyst.wrong_arity`. A value for an
  integer parameter must be a whole number, and every value must be finite
  (`catalyst.llvm_argument_invalid`).
- `--out` obeys the path rule of §0 (`out`, `module`, `rules`, `source` and
  `artifact` are path flags). A refused command writes nothing.
- `--mode` forces a mode. Without it CGC chooses: `reverse` when the engine
  accepts the function in reverse mode, `forward` otherwise (a function with a
  loop is differentiated forward, because this engine's reverse mode refuses a
  back edge). The chosen mode is recorded in the artifact. A forced mode the
  engine refuses is refused with the engine's own code (for example
  `catalyst.cyclic_control_flow`); a mode is never silently swapped. The mode
  changes nothing about validation: the same points, the same tolerance.
- `--rules` registers custom derivative rules, §12.5.
- `--source` names the original program the module was compiled from (for
  example the `.rs` or `.c` file). It is read only to be digested: its SHA-256
  and its file name (base name only, no directory) go into the provenance.
  Nothing else is done with it. Without it `source_digest` is `null`.

Output, on success:

```json
{"ok":true,"out":"DIR","backend":"cgc","mode":"reverse","function":"heat",
 "inputs":["x","y"],"at":{"x":1.5,"y":2},
 "value":…,"gradient":{"x":…,"y":…},"finite":true,
 "validation":{"points":5,"passed":5,"relative_tolerance":1e-6,"absolute_tolerance":1e-9},
 "assurance":{"computation_kind":"llvm","differentiation_backend":"cgc","validated":true,"portable_export":false},
 "files":["derivative.json","module.ll","primal.cir","derivative.cir","fixtures.json","validation-report.md"]}
```

`at` carries every parameter; `gradient` carries the named inputs. Non-finite
numbers are `null` with `"finite":false`, as in §2.

### 12.3 The IR CGC accepts (phase 1)

The subset is exactly what an optimising compiler emits for scalar numerical
code, and it is stated so that a refusal can name what fell outside it:

- Types: `double`, `i1`, `i8`, `i16`, `i32`, `i64`. A function returns
  `double`. Pointers, vectors, structs, arrays, `float`, `half`, `fp128` are
  outside the subset.
- Real arithmetic: `fadd`, `fsub`, `fmul`, `fdiv`, `fneg`; comparisons `fcmp`
  with the ordered predicates `oeq one olt ole ogt oge` and the unordered
  `une ueq ult ule ugt uge` (unordered means "or either side is NaN", which is
  how rustc spells a swapped `>=`; `ord`, `uno`, `true` and `false` are
  outside the subset); `select`.
- Integer arithmetic on the integer types: `add sub mul sdiv udiv srem urem and
  or xor shl lshr ashr`, `icmp` with every predicate, `zext sext trunc`,
  `sitofp uitofp fptosi fptoui`.
- Control flow: `br` (both forms), `phi`, `ret double`. Loops are accepted.
  `switch`, `unreachable`, `invoke` and `indirectbr` are outside the subset.
- Calls: the math intrinsics `llvm.{sin,cos,exp,log,sqrt,pow,fabs,maxnum,minnum,
  maximumnum,minimumnum,maximum,minimum}.f64`
  and the libm names `sin cos tan exp log sqrt tanh sinh cosh fabs erf pow
  atan2 fmax fmin` when declared; `llvm.assume`, `llvm.lifetime.*`,
  `llvm.dbg.*` and `llvm.experimental.noalias.scope.decl` are accepted and
  ignored. A call to a function **defined** in the module is differentiated
  through it (recursion is outside the subset). A call to any other declared
  function is `catalyst.opaque_call` naming the callee, and the module is not
  executed: a program that calls `@system`, `@getenv`, `@fopen` or anything
  else the engine cannot see through is refused before any evaluation.
- Memory (`alloca`, `load`, `store`, `getelementptr`) is outside the subset
  in this phase. The engine has the instructions; the lowering of LLVM's
  typed, byte-addressed memory onto them is phase 2 and is not claimed.
- Tolerated and ignored wherever LLVM writes them: fast-math flags, `nuw nsw
  exact disjoint nneg inbounds`, `tail musttail notail`, parameter and
  function attributes (`noundef`, `#0`, `unnamed_addr`, `dso_local`,
  linkage), `align`, trailing `!metadata`, comments, `source_filename`,
  `target datalayout`, `attributes #n = { … }`, and every `!n = …` line. Real
  constants in decimal (`8.000000e-01`) and hexadecimal (`0x3FE999999999999A`)
  form are both read.

Anything outside the subset is `catalyst.llvm_unsupported`, and the detail
names the opcode or type and the **line number** in the module. A module that
does not parse at all is `catalyst.llvm_syntax` with a line number. Nothing
outside the subset is guessed at.

### 12.4 The artifact

`--out DIR` receives exactly these files, byte-deterministic for the same
inputs, containing no timestamp, absolute path, home directory or environment
value (the compiler's own version string from `!llvm.ident` is copied
verbatim, because it is provenance, not a clock):

| File | Content |
| --- | --- |
| `derivative.json` | the artifact document below |
| `module.ll` | the input module, byte for byte |
| `primal.cir` | the primal in Catalyst's IR after lowering and optimisation, as `ir::print_func` writes it |
| `derivative.cir` | the derivative function in Catalyst's IR, the same printer |
| `fixtures.json` | `{"schema":"catalyst.derivative-fixtures.v1","function","inputs","tolerance":{"relative":1e-6,"absolute":1e-9},"cases":[…]}`; each case `{"case":"…","at":{…},"value":…,"gradient":{…},"estimate":{…},"agrees":true}` with `estimate` the verifier's finite-difference gradient at that point |
| `validation-report.md` | plain text: the function, the mode, every case with both gradients, the provenance chain, a `## Remaining uncertainty` section, and the sentence `The derivative was produced by the Catalyst Gradient Compiler and checked by Catalyst's own verifier; neither one declares the other correct.` verbatim |
| `rules.json` | only when `--rules` was given: the rules, as read |

`derivative.json`:

```json
{"schema":"catalyst.derivative.v1",
 "backend":"cgc","mode":"reverse",
 "function":"heat","inputs":["x","y"],"outputs":["heat"],
 "parameters":[{"name":"x","type":"double"},{"name":"y","type":"double"}],
 "source_digest":null,
 "source_name":null,
 "compiler_ir_digest":"<sha256 of module.ll>",
 "primal_ir_digest":"<sha256 of primal.cir>",
 "derivative_digest":"<sha256 of derivative.cir>",
 "validation_digest":"<sha256 of fixtures.json>",
 "rules_applied":[],
 "provenance":{"compiler":"rustc version …","target_triple":"x86_64-unknown-linux-gnu","cgc_version":"0.1.0","mode":"reverse","inputs":["x","y"],"output":"heat"},
 "assurance":{"computation_kind":"llvm","differentiation_backend":"cgc","validated":true,"portable_export":false},
 "gradient_validation":{"points":5,"passed":5,"relative_tolerance":1e-6,"absolute_tolerance":1e-9}}
```

- Every digest is SHA-256, 64 lowercase hex characters, computed in this
  crate (no dependency). `compiler_ir_digest` is over `module.ll` exactly as
  read; `source_digest` over `--source` when given, else `null`.
- `provenance.compiler` is the string inside `!llvm.ident` when the module has
  one, else `"unknown"`; `target_triple` likewise from `target triple`.
  `cgc_version` is the crate version.
- `assurance` makes the two lanes visibly different. An LLVM derivative is
  `portable_export:false`: there is no Go or R export of it in this phase, and
  `catalyst export` of one refuses (`catalyst.portable_export_unavailable`).

### 12.5 Validation is Catalyst's, not CGC's

CGC does not declare itself correct. For every derivative, Catalyst's verifier
evaluates the **primal** (lowered from the same module, executed by the
engine's interpreter, the derivative transform nowhere in the path) and forms
central finite-difference estimates of each requested partial at the declared
point and at further points around it: for each named input, that input moved
up and down by one per cent of its value (or by 0.01 when it is zero), the
others held at the declared point. So a request over `n` inputs is validated
at `1 + 2n` points. Each estimate must agree with the derivative within
relative `1e-6` or absolute `1e-9`. If every point agrees, `validated` is
`true` and the artifact is written. If any point disagrees the whole command
is refused as **`catalyst.derivative_disagrees`**, the detail names the input,
the point, the derivative's number and the estimate's number, and nothing is
written. An artifact with `validated:false` is never produced.

**Custom rules.** Engineering code contains functions whose derivative the
engineer already knows. `--rules FILE` reads
`{"schema":"catalyst.derivative-rules.v1","rules":[{"function":"special_lookup","derivative":"special_lookup_gradient"}]}`.
A rule applies to a function of exactly one `double` parameter returning
`double`; both names must be `define`d in the module (`catalyst.rule_invalid`
otherwise, naming what is missing or what has the wrong shape). Where the
rule applies, a call `y = f(u)` is differentiated as `dy/du = g(u)` and `f` is
not inlined. The names of the rules that applied are recorded in
`rules_applied`. **A rule is not trusted because a developer supplied it**:
the verifier runs the primal, which calls the real `f`, so a wrong `g` is a
`catalyst.derivative_disagrees` like any other wrong derivative.

### 12.6 `catalyst differentiate run --artifact DIR --at v1,…,vn`

Executes an artifact again, without the module being named. It reads
`derivative.json` and `module.ll` from `DIR`, checks that `module.ll` still
has `compiler_ir_digest`, regenerates the derivative in the recorded mode with
the recorded rules, checks that the regenerated `derivative.cir` still has
`derivative_digest`, and only then evaluates at the point:

`{"ok":true,"artifact_verified":true,"backend":"cgc","mode":"reverse","function":"heat","at":{…},"value":…,"gradient":{…},"finite":true}`.

Either digest failing is `catalyst.artifact_tampered`, naming which one, and
nothing is evaluated. The numbers at the artifact's own point are identical to
the numbers `differentiate llvm` printed.

### 12.7 `catalyst differentiate native --problem FILE --out DIR`

The same artifact from the expression lane, so that a reader can see one
artifact model over both. `backend:"native"`, `mode:"reverse"`,
`assurance:{"computation_kind":"catalyst-ir","differentiation_backend":"native","validated":true,"portable_export":true}`.
Files: `derivative.json`, `problem.json` (the validated document, as
`Problem::to_json` writes it), `primal.cir`, `derivative.cir`, `fixtures.json`,
`validation-report.md`. `source_digest` is over the `function` text and
`compiler_ir_digest` over `primal.cir`; the `provenance.compiler` is
`"catalyst"`. The `value` and `gradient` are exactly `catalyst eval`'s. The
same verifier runs, at the same points, with the same tolerance.
`differentiate run` on a native artifact reads `problem.json` in place of
`module.ll`.

### 12.8 `catalyst.problem.v2`: a computation, not only a function

```json
{"schema":"catalyst.problem.v2","name":"heat","goal":"…",
 "computation":{"kind":"llvm","module":"heat.ll","function":"heat"},
 "inputs":{"x":1.5,"y":2.0},
 "domains":{"x":{"min":0.1,"max":10,"unit":""},"y":{"min":0.1,"max":10,"unit":""}}}
```

- `computation.kind` is `catalyst-expression` (then `computation.function` is
  the §1 expression and everything behaves as a v1 document) or `llvm` (then
  `computation.module` is a path under the path rule and
  `computation.function` a define in it). Any other kind, including `rust`,
  `c` and `cpp`, is `catalyst.computation_kind_unsupported`, and the remedy
  says to compile the program to LLVM IR text (`rustc --emit=llvm-ir`,
  `clang -S -emit-llvm`) and use kind `llvm`. Compiling source is not built
  and is not claimed.
- For kind `llvm`, `inputs` and `domains` name the `double` parameters to
  differentiate; any integer parameter of the function is given its value in
  `computation.fixed` (`{"n":5}`) and is held fixed. A parameter in neither is
  `catalyst.problem_incomplete`.
- A v1 document is unchanged in every way. `catalyst eval` of a v2 document
  prints the §2 object plus `"assurance":{…}` and, for kind `llvm`,
  `"validation":{…}`; an LLVM derivative that fails validation refuses
  `catalyst.derivative_disagrees` exactly as `differentiate llvm` does.
- `catalyst export go|r` of a v2 document of kind `llvm` is
  `catalyst.portable_export_unavailable` and writes nothing. Of kind
  `catalyst-expression` it is the ordinary export.

### 12.9 The tool surface learns the capability

`differentiate_llvm` joins the tool list of §5, so the list is five:
`discover`, `validate_problem`, `evaluate`, `export_go`, `differentiate_llvm`;
the adapter's capability list of §6 is that same list. Request arguments:
`{"module":"<the .ll text>","function":"heat","inputs":["x","y"],"at":[1.5,2],"rules":{…optional…}}`.
Response `result`: `{"backend","mode","function","inputs","at","value","gradient","finite","validation","assurance"}`.
The tool writes no file. The authority refusals of §5 apply unchanged: a
model may ask for a derivative; it may not set the tolerance the verifier
uses, and a request carrying a top-level `tolerance` is
`catalyst.authority_refused` here as everywhere.

### 12.10 Offline, and inside the engine

CGC runs no program, opens no socket, reads no environment. The program under
differentiation is executed only by the engine's own interpreter, which has no
instruction that can reach the operating system: a step budget of 2^26
instructions per evaluation (`catalyst.llvm_run_faulted` names the fault, for
example a loop that did not terminate within the budget for the given inputs,
and the refusal arrives in seconds rather than never) and a bounded call depth.
`src/gradient_compiler/**` contains none of `process::Command`, `std::net`,
`std::env` or `std::fs`: every path is read and every file written by the
command layer through `paths`, as for every other command. With an empty
environment `differentiate llvm` works exactly as with a full one.

### 12.11 What this phase does not claim

- Compiling Rust, C or C++ source: the user runs their compiler; kind `rust`
  refuses and says so.
- Memory, arrays, structs, vectors: phase 2.
- A Go or R export of an LLVM derivative: `portable_export:false`, and the
  export refuses.
- Automatic mode selection changing the result: it does not; both modes are
  validated identically.
