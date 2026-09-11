//! The `catalyst` binary.
//!
//! One command, several subcommands, and one shape of answer: a JSON object on
//! stdout, `"ok":true` and exit 0 when the command did what was asked,
//! `{"ok":false,"code":…,"detail":…,"remedy":…}` and exit 2 when it refused.
//! `--help` prints plain usage text and exits 0, because asking what a command
//! accepts is not a mistake.
//!
//! # Exit 101 never happens
//!
//! A panic is the one answer a caller cannot act on: no code to match, no
//! remedy to follow, and a message on the wrong stream. Every path through
//! this binary ends in one of the three above, and the hostile-input oracle
//! exists to keep it that way.
//!
//! # Every path a user names is checked first
//!
//! Before a command runs -- including a command this version has not built
//! yet -- every flag naming a path goes through [`catalyst::paths::resolve`].
//! A refusal that arrives after the directory was created would be a refusal
//! that did not prevent anything.

#![forbid(unsafe_code)]

use catalyst::cli::{self, Args, Refused, PATH_FLAGS};
use catalyst::export;
use catalyst::json::{obj, s, Json};
use catalyst::paths;
use catalyst::problem;

/// What a command produced: usage text, or the one JSON object.
enum Output {
    Text(String),
    Json(String),
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(Output::Text(text)) => print!("{text}"),
        Ok(Output::Json(text)) => println!("{text}"),
        Err(refused) => {
            println!("{}", refused.to_json());
            std::process::exit(2);
        }
    }
}

fn run(argv: &[String]) -> Result<Output, Refused> {
    let Some(command) = argv.first() else {
        return Ok(Output::Text(cli::TOP_USAGE.to_owned()));
    };
    if command == "--help" || command == "-h" {
        return Ok(Output::Text(cli::TOP_USAGE.to_owned()));
    }
    let args = Args::read(&argv[1..]);
    if args.help {
        return Ok(Output::Text(cli::usage(command).to_owned()));
    }
    // Every path a user named, before any command does anything with any of
    // them. A command that read its input first would refuse a bad output
    // path only after the work, and one that created its output first would
    // refuse it only after the directory existed.
    check_paths(&args)?;
    match command.as_str() {
        "eval" => eval(&args),
        "export" => export_command(&args),
        "tui" => catalyst::tui::run(&args).map(Output::Json),
        "tools" => catalyst::tools::run(&args).map(Output::Json),
        "ai" => catalyst::ai::run(&args).map(Output::Json),
        "adapter" => catalyst::adapter::run(&args).map(Output::Json),
        "rehearse" => catalyst::rehearse::run(&args).map(Output::Json),
        "differentiate" => catalyst::differentiate::run(&args).map(Output::Json),
        other => Err(Refused::usage(format!(
            "`{}` is not a catalyst command",
            quoted(other)
        ))),
    }
}

/// The path rule over every flag that names a place, in the order the flags
/// were written.
fn check_paths(args: &Args) -> Result<(), Refused> {
    for (flag, value) in args.pairs() {
        if PATH_FLAGS.contains(&flag.as_str()) {
            paths::resolve(value)?;
        }
    }
    Ok(())
}

fn eval(args: &Args) -> Result<Output, Refused> {
    let named = args.required("problem", "eval")?;
    let problem = problem::parse(&paths::read_input(named)?)?;
    // The same computation, and the same spelling of it, that the `evaluate`
    // tool returns: a caller must not be able to tell the two apart.
    let mut members = vec![("ok", Json::Bool(true)), ("name", s(&problem.name))];
    members.extend(catalyst::tools::evaluation(&problem)?);
    Ok(Output::Json(obj(members).render()))
}

fn export_command(args: &Args) -> Result<Output, Refused> {
    // Two targets, one shape. The language is a bare word rather than a flag
    // because it selects the whole rest of the command, not a detail of it.
    let (language, files) = if args.word("go") {
        ("go", export::FILES)
    } else if args.word("r") {
        ("r", export::R_FILES)
    } else if let Some(spelled) = ["go", "r"].into_iter().find(|name| args.has(name)) {
        // `--go` and `--r` are the flag spelling of the word: refused by
        // name, rather than accepted as if the word had been written.
        return Err(Refused::usage(format!(
            "`catalyst export` takes its target as a word, not a flag: write \
             `catalyst export {spelled} --problem FILE --out DIR`, not `--{spelled}`"
        )));
    } else {
        return Err(Refused::usage(
            "`catalyst export` needs a target: `catalyst export go --problem FILE --out DIR` \
             or `catalyst export r --problem FILE --out DIR`",
        ));
    };
    let command = format!("export {language}");
    let named = args.required("problem", &command)?;
    let out_named = args.required("out", &command)?;
    // The problem is validated before the output directory is touched, so a
    // refused document leaves nothing behind.
    let problem = problem::parse(&paths::read_input(named)?)?;
    if let Some(refused) = problem.portable_export_unavailable() {
        return Err(refused);
    }
    let out = paths::prepare_out_dir(out_named)?;
    let cases = match language {
        "r" => export::write_r_export(&problem, &out)?,
        _ => export::write_go_export(&problem, &out)?,
    };
    let document = obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(out_named)),
        ("files", Json::Arr(files.iter().map(|f| s(f)).collect())),
        ("cases", count(cases)),
    ]);
    Ok(Output::Json(document.render()))
}

fn count(n: usize) -> Json {
    Json::Num(n as f64)
}

/// A word from the command line, in a form a message can carry: no control
/// characters, and cut to a length that cannot flood a terminal.
fn quoted(word: &str) -> String {
    let mut out: String = word.chars().filter(|c| !c.is_control()).take(32).collect();
    if word.chars().count() > 32 {
        out.push('…');
    }
    out
}
