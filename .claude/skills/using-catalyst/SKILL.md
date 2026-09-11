---
name: using-catalyst
description: Use when a task needs a numerical computation measured, differentiated, checked or exported with Catalyst — evaluating a problem document, exporting Go or R, differentiating a compiled program through the Catalyst Gradient Compiler, driving the terminal front end headlessly, or calling the tool surface.
---

# Using Catalyst

Catalyst prints one JSON object per command. `"ok":true` is a measurement;
`"ok":false` carries `code` and `remedy` and wrote nothing. Follow the remedy.
Paths are relative and inside the current directory. Output directories must
be empty or absent.

## Recipe 1 — measure a function and what moves it

1. Write `problem.json`:
   ```json
   {"schema":"catalyst.problem.v1","name":"spring","goal":"settle fast without overshoot",
    "function":"func spring(k, c) = 8 / c + 100 * exp(0 - 3.141592653589793 * c / sqrt(4 * k - c * c))",
    "inputs":{"k":12.0,"c":1.5},
    "domains":{"k":{"min":1.0,"max":100.0,"unit":"N/m"},"c":{"min":0.1,"max":1.9,"unit":"N*s/m"}}}
   ```
   Every parameter of `function` needs an entry in both `inputs` and
   `domains`; `name` is `[a-z][a-z0-9_-]{0,31}`.
2. `catalyst eval --problem problem.json`
3. Read `value` and `gradient` (keyed by parameter name). `finite:false`
   means a number was not finite and is written as `null`.

## Recipe 2 — export and check it without Catalyst

1. `catalyst export go --problem problem.json --out export`
2. `cd export && go vet ./... && go build -o exp . && ./exp fixtures.json`
3. Every case in the output is either a value with a gradient, `refused` (out
   of domain, as declared), or `nonfinite`. The fixtures carry the engine's
   own numbers with a declared tolerance.
4. For R: `catalyst export r --problem problem.json --out export-r`, then
   `Rscript --vanilla test_function.R` inside it.

## Recipe 3 — differentiate a program the user already has

1. Compile to LLVM IR text with the user's compiler; Catalyst never runs one:
   `rustc --emit=llvm-ir -O -C panic=abort -C debuginfo=0 -C no-vectorize-slp -C no-vectorize-loops model.rs -o model.ll`
   (or `clang -S -emit-llvm -O2 model.c -o model.ll`).
2. `catalyst differentiate llvm --module model.ll --function drag --inputs velocity,density --at 30,1.225,4 --out derivative`
   — `--inputs` are the `double` parameters to differentiate with respect to,
   `--at` is a value for **every** parameter in declaration order.
3. Read `gradient`, `mode` (reverse, or forward for a loop), `validation`
   (`passed` must equal `points`) and `assurance`.
4. `derivative/derivative.json` carries the digest chain and provenance
   (add `--source model.rs` to record the source digest);
   `catalyst differentiate run --artifact derivative --at 30,1.225,4` executes
   the artifact again for any in-domain point and refuses
   `catalyst.artifact_tampered` if `module.ll`, the document, `primal.cir`,
   `derivative.cir` or `fixtures.json` changed.
5. If the program calls a function whose derivative the user knows, write
   `rules.json`:
   `{"schema":"catalyst.derivative-rules.v1","rules":[{"function":"lookup","derivative":"lookup_gradient"}]}`
   and add `--rules rules.json`. The rule is verified like any derivative.
   Keep the call alive through the optimiser with `#[inline(never)]` on the
   callee, otherwise it is inlined and `rules_applied` is `[]`.
   An LLVM-lane artifact is not portable (`portable_export: false`): to hand
   the team Go or R, write the formula as a problem file (Recipe 1).
6. Refusals: `catalyst.unknown_name` lists the parameter names the IR has
   (rustc leaves one unnamed, `0`, when the body only copies it into a `mut`
   local: use that number); `catalyst.llvm_unsupported` names the line
   (memory and arrays are outside this phase); `catalyst.opaque_call` names an undefined callee;
   `catalyst.derivative_disagrees` means the verifier did not confirm the
   derivative — report both numbers; never widen anything.

## Recipe 4 — drive the terminal front end headlessly

1. `catalyst tui --describe` — the panes, steps and keys, as JSON. (A person
   running `catalyst tui` gets a frame sized to their terminal; headless runs
   are 80 by 24 unless `--width`/`--height` say otherwise.)
2. Write `keys.txt`, one event per line:
   ```
   text:func f(x, y) = x * y + sin(x)
   enter
   text:0.7
   tab
   text:-3
   tab
   text:3
   tab
   text:
   tab
   text:1.3
   tab
   text:-2
   tab
   text:2
   tab
   text:
   enter
   enter
   enter
   text:export
   enter
   ```
   (Goal, then per parameter value / min / max / unit, then Run, Results,
   Export.)
3. `catalyst tui --headless --keys keys.txt --transcript frames.txt --out export`
4. Parse `frames.txt`: each frame starts `--- frame N (step: …, state: …) ---`.
   A result exists only at `state: measured`; `failed` and `inconclusive` are
   the other outcomes. `Problem:` / `Next step:` lines say what to fix.
   The transcript is plain text with no escape sequences.
5. `ctrl-s` saves to `--save FILE`; `--resume FILE` restores the function,
   every field and the state, and opens at Run: send the run key once to
   reproduce the numbers before reading Results.

## Recipe 5 — call Catalyst as a tool

1. `catalyst tools discover` — five tools with schemas.
2. Write `request.json`:
   `{"schema":"catalyst.tool-request.v1","tool":"evaluate","arguments":{"problem":{…}}}`
   (`export_go` also takes `"out"`; `differentiate_llvm` takes `module` text,
   `function`, `inputs`, `at`, optional `rules`).
3. `catalyst tools call --request request.json` (`--request -` for stdin).
4. Never send `set_acceptance`, `set_tolerance`, `approve_result`, or a
   top-level `acceptance`/`tolerance`/`approve_own_result`: refused as
   `catalyst.authority_refused`, by design.

## What not to do

- Do not edit `tests/` or a tolerance to make something pass.
- Do not run the user's compiled program to check Catalyst; Catalyst's
  verifier already checked it inside its interpreter.
- Do not send anything to a network; Catalyst never does.
- Do not report a number you did not read from a `"ok":true` object.
