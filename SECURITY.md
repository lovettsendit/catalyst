# Security

## Reporting a problem

Report a suspected vulnerability privately through this repository's GitHub
private vulnerability reporting ("Report a vulnerability" under the Security
tab), not in a public issue. You will get an acknowledgement, and a fix or a
statement of why no fix is needed, before anything is published.

## Posture

- **No third-party code.** Catalyst has zero dependencies: `[dependencies]` is
  empty, there are no build or dev dependencies and no `build.rs`. What you
  compile is what is in this tree.
- **No unsafe code.** The crate root carries `#![forbid(unsafe_code)]`; the
  exported Go is pure Go with no `cgo` and no `unsafe`.
- **No network.** Catalyst never connects to anything you did not name, and
  needs no cloud. Evaluation, export, differentiation and the terminal flow
  open no socket at all. The one command that connects is `catalyst rehearse`,
  and it connects only to a local target you name: a Unix socket under the
  current directory, `127.0.0.1`, `localhost` or `[::1]`; any other target is
  refused before a request is made. The optional AI layer runs a local
  command-line program and never a network client of its own.
- **Rehearsal on a local stand-in only.** Failure rehearsal runs a local copy
  of a service, never a public interface, and replays sanitised fixtures
  rather than live traffic.
- **Two external programs, and no shell.** The only programs Catalyst ever
  starts are `stty` (the interactive terminal flow, to read the terminal's
  size and its keys) and the AI
  command you configured (`catalyst ai` only). Neither is started through a
  shell, and no user-supplied string ever reaches one.
- **No credential, from anywhere.** Catalyst has no API-key setting, no
  provider network client and no account of its own, so there is no credential
  for it to mishandle. The AI feature runs a command-line program you have
  already signed into with whatever plan you pay for, and reads its answer. The
  only environment values Catalyst reads at all are `CATALYST_AI_COMMAND`,
  `CATALYST_AI_MODEL`, `CATALYST_AI_ARGS` and `PATH`. A credential that happens
  to be in the environment is never read, never placed in a request, never
  written to disk, never logged and never echoed in an error.
- **A program you differentiate never runs on your machine.** The Catalyst
  Gradient Compiler reads the LLVM IR text your own compiler wrote and executes
  the program only inside Catalyst's interpreter, which has no instruction that
  can reach the operating system. A call to anything outside the module is
  refused before evaluation, a program that does not terminate is stopped by a
  step budget, and nothing about it leaves the machine: differentiation does
  not require sending source code anywhere.
- **Writes stay inside the paths you name.** Every command writes only under
  the path you give it, inside the current directory, and a refused command
  writes nothing.
- **Hostile input is refused, not crashed on.** Malformed goals, expressions
  and fixtures produce a stated refusal with the reason.
