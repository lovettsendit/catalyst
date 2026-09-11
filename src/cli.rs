//! What every subcommand of the `catalyst` binary shares: one refusal shape,
//! one argument reader, and the usage text.
//!
//! # One shape for every answer
//!
//! Success is `{"ok":true, …}` and exit 0. A refusal is
//! `{"ok":false,"code":…,"detail":…,"remedy":…}` and exit 2. Nothing else
//! happens, on any input: a caller parses one object and branches on one
//! field, and a caller that is a program never has to tell a crash from a
//! considered no.
//!
//! # Why the arguments are read by hand
//!
//! Because the crate has no dependencies, and because the grammar is small
//! enough that a hand-written reader is shorter than the configuration an
//! argument crate would need. Flags are long-form only. A value that looks
//! like another flag is refused rather than swallowed, which is the mistake
//! that turns a typo into a file written somewhere unexpected.

use crate::refuse::Refusal;

/// A refusal on its way to stdout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub code: String,
    pub detail: String,
    pub remedy: String,
}

impl Refused {
    pub fn new(code: &str, detail: impl Into<String>, remedy: impl Into<String>) -> Refused {
        Refused {
            code: code.to_owned(),
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }

    /// The refusal as the one object every command prints.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\"ok\":false,\"code\":");
        out.push_str(&crate::json::s(&self.code).render());
        out.push_str(",\"detail\":");
        out.push_str(&crate::json::s(&self.detail).render());
        out.push_str(",\"remedy\":");
        out.push_str(&crate::json::s(&self.remedy).render());
        out.push('}');
        out
    }

    /// The refusal every command that is registered but not yet built prints.
    pub fn not_implemented(what: &str) -> Refused {
        Refused::new(
            "catalyst.not_implemented",
            format!("{what} is not built in this version"),
            format!(
                "run `catalyst {what} --help` to see what it will accept, and use \
                     `catalyst eval` or `catalyst export go` in the meantime"
            ),
        )
    }

    pub fn usage(detail: impl Into<String>) -> Refused {
        Refused::new(
            "catalyst.usage",
            detail,
            "run `catalyst --help`, or `catalyst <command> --help`, for the accepted arguments",
        )
    }
}

/// An engine refusal, in the shape the command line prints. The code and the
/// remedy are the engine's own: a caller matching on `catalyst.unknown_name`
/// gets the same string whether it called the crate or the binary.
impl From<Refusal> for Refused {
    fn from(refusal: Refusal) -> Refused {
        Refused::new(
            refusal.code(),
            refusal.cause.to_string(),
            refusal.cause.remedy(),
        )
    }
}

/// The arguments of one subcommand: flags with values, and bare words.
pub struct Args {
    pairs: Vec<(String, String)>,
    flags: Vec<String>,
    /// The bare words alone, without the `--` flags that took no value, so
    /// a command can tell `export go` from `export --go`.
    words: Vec<String>,
    pub help: bool,
}

impl Args {
    /// Read `--name value` pairs and bare `--name` flags.
    ///
    /// A flag whose value is missing, or whose value begins with `--`, is a
    /// bare flag: `--resume` is one on `ai propose` and takes a file on
    /// `tui`, and the command that knows which decides. The exceptions are
    /// [`VERBATIM_VALUE_FLAGS`].
    pub fn read(argv: &[String]) -> Args {
        let mut pairs = Vec::new();
        let mut flags = Vec::new();
        let mut words = Vec::new();
        let mut help = false;
        let mut i = 0;
        while i < argv.len() {
            let arg = &argv[i];
            if arg == "--help" || arg == "-h" {
                help = true;
                i += 1;
                continue;
            }
            if let Some(name) = arg.strip_prefix("--") {
                let verbatim = VERBATIM_VALUE_FLAGS.contains(&name);
                match argv.get(i + 1) {
                    Some(v) if verbatim || !v.starts_with("--") => {
                        pairs.push((name.to_owned(), v.clone()));
                        i += 2;
                    }
                    // A flag that always takes a value, given none, has been
                    // given an empty one: the command that reads it says so,
                    // rather than this reader quietly leaving it out and
                    // running whatever the default was.
                    None if verbatim => {
                        pairs.push((name.to_owned(), String::new()));
                        i += 1;
                    }
                    _ => {
                        flags.push(name.to_owned());
                        i += 1;
                    }
                }
                continue;
            }
            flags.push(arg.clone());
            words.push(arg.clone());
            i += 1;
        }
        Args {
            pairs,
            flags,
            words,
            help,
        }
    }

    /// The value of `--name`, if it was given one.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The value of `--name`, or a refusal naming the flag and the command.
    pub fn required(&self, name: &str, command: &str) -> Result<&str, Refused> {
        self.value(name).ok_or_else(|| {
            Refused::usage(format!(
                "`catalyst {command}` needs --{name}, and it was not given a value"
            ))
        })
    }

    pub fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name) || self.value(name).is_some()
    }

    /// Whether `name` was given as a bare word rather than as a flag with a
    /// value.
    ///
    /// Subcommands are bare words, and some of them share a name with a flag:
    /// `catalyst rehearse report --compare FILE` names the *report*
    /// subcommand and takes a comparison, and `catalyst rehearse compare
    /// --held-out FILE` names *compare* and takes a held-out set. Dispatching
    /// on [`Args::has`] would read those the wrong way round, so a dispatcher
    /// asks this instead.
    pub fn bare(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    /// Whether `name` was written as a plain word, with no `--` in front of
    /// it. `catalyst export go` is one; `catalyst export --go` is not, and a
    /// command that takes a form as a word can say so rather than accept the
    /// misspelling quietly.
    pub fn word(&self, name: &str) -> bool {
        self.words.iter().any(|w| w == name)
    }

    /// Every `--name value` pair, in the order they were written. The path
    /// rule is applied over this, so a flag naming a place outside the
    /// current directory is refused before the command runs at all.
    pub fn pairs(&self) -> &[(String, String)] {
        &self.pairs
    }
}

/// Flags whose next argument is their value even when it begins with `--`.
///
/// The general rule -- a value that looks like another flag is not swallowed
/// -- is what stops a typo like `--out --problem p.json` from creating a
/// directory called `--problem`. `--provider-args` is the one flag whose value
/// is *made of* arguments: `--provider-args "--prompt-stdin"` is exactly what
/// somebody on a plan whose CLI takes no model writes, and applying the
/// general rule to it would ignore what they asked for and silently run the
/// default argument list against their CLI instead.
pub const VERBATIM_VALUE_FLAGS: &[&str] = &["provider-args"];

/// Flags whose value is a path the user named, and which therefore go through
/// [`crate::paths::resolve`] before anything happens. `docs/interface.md` §0
/// lists them; they are checked on every command that accepts them, including
/// the ones that then refuse for another reason.
pub const PATH_FLAGS: &[&str] = &[
    "out",
    "dir",
    "transcript",
    "save",
    "state",
    "trace",
    "patterns",
    "result",
    "scenario",
    "held-out",
    "compare",
    "keys",
    "request",
    "context",
    "problem",
    "resume",
    "module",
    "rules",
    "source",
    "artifact",
];

pub const TOP_USAGE: &str = "\
catalyst -- turn a numerical goal into a checked export

usage: catalyst <command> [options]

commands:
  eval           evaluate a problem and report its value and gradient
  export         write a self-contained export of a problem
  differentiate  the Catalyst Gradient Compiler: a derivative artifact from
                 LLVM IR or from a problem, checked by Catalyst's verifier
  tui            the guided terminal flow
  tools          the tool surface a model calls
  ai             propose a problem with the configured AI command
  adapter        the local-adapter boundary
  rehearse       failure rehearsal against a local stand-in

every command prints one JSON object on stdout. a refusal carries
ok:false, a stable code, a detail and a remedy, and exits with code 2.

run `catalyst <command> --help` for the arguments of one command.
";

/// The usage text of one subcommand. Plain text, exit 0: a caller asking what
/// a command accepts is not making a mistake, and should not be handed a
/// refusal for asking.
pub fn usage(command: &str) -> &'static str {
    match command {
        "eval" => {
            "\
usage: catalyst eval --problem FILE

reads a catalyst.problem.v1 or catalyst.problem.v2 document and prints
{\"ok\":true,\"name\":...,\"value\":...,\"gradient\":{...},\"finite\":true,\"cost\":{...}}

a v2 document names a computation of kind catalyst-expression or llvm; the
answer then carries an `assurance` block, and for kind llvm a `validation`
block from Catalyst's verifier.

options:
  --problem FILE   the problem to evaluate
"
        }
        "differentiate" => {
            "\
usage: catalyst differentiate llvm --module FILE.ll --function NAME --inputs a,b
                                   --at v1,...,vn --out DIR [--mode forward|reverse]
                                   [--rules FILE] [--source FILE]
       catalyst differentiate native --problem FILE --out DIR
       catalyst differentiate run --artifact DIR --at v1,...,vn
       catalyst differentiate --describe

the Catalyst Gradient Compiler. `llvm` reads the textual LLVM IR your own
compiler wrote (`rustc --emit=llvm-ir`, `clang -S -emit-llvm`), lowers the
named function to Catalyst's IR, differentiates it, and writes a derivative
artifact -- but only after Catalyst's verifier has checked every partial
against central finite differences over the primal, at the point and around
it. A derivative the verifier does not confirm is refused, and nothing is
written. `native` writes the same artifact from a problem of the expression
language. `run` executes an artifact again from its own files, refusing if
any of them was changed. `--describe` lists the backends and the validation,
running nothing and writing nothing.

options:
  --module FILE     the textual LLVM IR module (.ll), at most 4 MiB
  --function NAME   the define to differentiate, without the @
  --inputs a,b      the double parameters to differentiate with respect to;
                    the gradient is reported in this order
  --at v1,...,vn    a value for every parameter, in declaration order; an
                    integer parameter takes a whole number
  --out DIR         the artifact directory, created if absent and required to
                    be empty if it exists
  --mode MODE       force forward or reverse; without it reverse is used when
                    the engine accepts the function and forward otherwise
                    (a loop is differentiated forward); a forced mode the
                    engine refuses is refused with the engine's own code
  --rules FILE      a catalyst.derivative-rules.v1 document binding functions
                    of the module to their derivative functions; a rule is
                    checked by the verifier like any other derivative
  --source FILE     the original program, read only to record its name and
                    SHA-256 in the artifact's provenance
  --problem FILE    for `native`: the problem to differentiate
  --artifact DIR    for `run`: the artifact to execute again
  --describe        print the backends, the artifact schema and the
                    validation as JSON
"
        }
        "export" => {
            "\
usage: catalyst export go --problem FILE --out DIR
       catalyst export r  --problem FILE --out DIR

writes a self-contained export reproducing the problem's value and gradient,
with fixtures it is checked against and a validation report.

  go  a Go module that needs nothing outside the standard library
  r   base R that loads no package, by any spelling

both are generated from the same optimised, differentiated IR, so the two
exports of one problem are two printings of one computation.

options:
  --problem FILE   the problem to export
  --out DIR        the directory to write, created if absent and required to
                   be empty if it exists
"
        }
        "tui" => {
            "\
usage: catalyst tui [--keys FILE] [--headless] [--transcript FILE]
                    [--out DIR] [--save FILE] [--resume FILE]
                    [--width N] [--height N] [--states] [--describe]

the guided flow: Goal > Inputs > Run > Results > Export, shown as four panes
in one frame: catalyst, measurements, steps and keys.

options:
  --keys FILE        read key events from a script instead of the keyboard
  --headless         render plain text instead of ANSI
  --transcript FILE  append every frame to this file
  --out DIR          where the export step writes
  --save FILE        where ctrl-s writes the session state
  --resume FILE      restore a saved session state
  --width N          terminal columns to draw for (default 80)
  --height N         terminal rows to draw for (default 24)
  --states           print the five state labels as a JSON array
  --describe         print the layout and each step's keys as JSON, run
                     nothing and write nothing
"
        }
        "tools" => {
            "\
usage: catalyst tools discover
       catalyst tools call --request FILE

the tool surface, which is one list of five: discover, validate_problem,
evaluate, export_go, differentiate_llvm. `discover` is callable as well as
printable, so an agent that can make one call can find the other four.

options:
  --request FILE   the catalyst.tool-request.v1 document, or - for stdin
"
        }
        "ai" => {
            "\
usage: catalyst ai status [--command NAME] [--model NAME]
                          [--provider-args TEMPLATE]
       catalyst ai propose --goal TEXT --out FILE --state FILE
                           [--context PROBLEM] [--command PATH] [--model NAME]
                           [--provider-args TEMPLATE] [--transcript FILE]
                           [--resume]

runs a command-line tool you have already signed into, with the prompt on
standard input. no shell is involved, and only approved fields of the context
problem are ever sent.

catalyst holds no credential, has no account of its own and makes no provider
network call: whichever plan you pay for, it runs that plan's CLI and reads
what it prints.

options:
  --goal TEXT             what the proposed problem should capture, in prose
  --out FILE              where the proposed problem is written, inside the
                          current directory; nothing is written on a refusal
  --state FILE            where the goal and every attempt are kept, so an
                          interrupted call can be continued
  --context PROBLEM       an existing problem document whose validated fields
                          are sent with the goal, and nothing else of it
  --transcript FILE       where the exact prompt sent and the reply read are
                          written, for inspection
  --resume                continue the pending call the state file holds,
                          rather than starting the goal again
  --command PATH          the CLI to run (default `claude`, or CATALYST_AI_COMMAND)
  --model NAME            the model to ask for (default `claude-opus-5`, or
                          CATALYST_AI_MODEL)
  --provider-args TEMPLATE
                          that CLI's argument shape, split on whitespace, with
                          {model} replaced by the model name; a template with
                          no {model} is run without a model argument.
                          default `-p --model {model} --output-format json`,
                          or CATALYST_AI_ARGS. for example, another plan's CLI:
                          --provider-args \"exec --model {model} --json\"

`catalyst ai status` prints the exact argument list it would use.
"
        }
        "adapter" => {
            "\
usage: catalyst adapter conformance
       catalyst adapter translate --request FILE

local-adapter extension interface; live local-model compatibility untested

options:
  --request FILE   the catalyst.adapter-request.v1 document
"
        }
        "rehearse" => {
            "\
usage: catalyst rehearse standin --out DIR
       catalyst rehearse synth-trace --out FILE [--sessions N] [--seed S]
       catalyst rehearse import --trace FILE --out FILE
       catalyst rehearse init --dir R --target T
       catalyst rehearse status --dir R
       catalyst rehearse replay --dir R (--patterns FILE | --scenario FILE)
                                [--disturb KIND] [--requests N] --out FILE
       catalyst rehearse reduce --result FILE --dir R --out FILE
       catalyst rehearse held-out --out FILE [--seed S]
       catalyst rehearse compare --dir R --baseline T1 --candidate T2
                                 --scenario FILE --held-out FILE --out FILE
       catalyst rehearse report --compare FILE --out FILE

  --disturb KIND   burst, slow_dependency, timeout, malformed_input or none.
                   A reduced scenario records the disturbance that produced
                   the failure and replays under it, so leaving this out
                   replays the scenario as it failed rather than undisturbed.
  --requests N     how many requests to issue from an imported pattern set.

targets are local only: unix:<relative path>, http://127.0.0.1:PORT,
http://localhost:PORT or http://[::1]:PORT. anything else is refused before a
request is made.
"
        }
        _ => TOP_USAGE,
    }
}
