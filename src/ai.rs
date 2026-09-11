//! Catalyst's own AI: `catalyst ai status` and `catalyst ai propose`.
//!
//! `docs/interface.md` §5. One external command, run with the argument list
//! its own CLI expects and the prompt on standard input. No shell, no network
//! code, no credential handling of any kind.
//!
//! # Catalyst is bought from nobody
//!
//! It has no account, no API key setting and no provider client. What it has
//! is the ability to run a command-line program that the person using it has
//! *already signed into* -- a Claude Max seat through `claude`, a Codex Pro
//! seat through `codex`, any other plan through its own CLI -- and to read
//! what that program prints. The credential for that plan lives where the user
//! put it when they signed in, and Catalyst never goes looking for it.
//!
//! That is why the argument list is a template ([`arguments`]) and why the
//! answer reader accepts both an envelope and plain output ([`answer_text`]).
//! Every CLI has its own shape, and a tool that hard-coded one vendor's shape
//! would be a tool for that vendor's customers, however carefully it avoided
//! saying so.
//!
//! # The feature that is off says so, and everything else keeps working
//!
//! `catalyst ai status` exits 0 whether or not the command exists, because a
//! feature being unavailable is an answer, not a failure. Every manual command
//! -- `eval`, `export go`, `tui`, `tools` -- runs without ever looking for the
//! command at all: somebody with no account and no network has the whole
//! product except this one subcommand.
//!
//! # What may leave the process
//!
//! Exactly four things: the goal text the user typed, the discovery document
//! Catalyst itself generates, the validated fields of the problem named by
//! `--context`, and the instruction to answer with one `catalyst.problem.v1`
//! object. Nothing else. In particular nothing from the environment: this
//! module reads three variables -- [`COMMAND_VARIABLE`], [`MODEL_VARIABLE`]
//! and [`ARGS_VARIABLE`] -- which name the program to run, the model to ask
//! for and the shape of its command line, and none of which is ever placed in
//! a prompt. No other variable is read at all, and no credential is read from
//! anywhere: not the environment, not a file, not a configuration directory.
//!
//! The context problem is not copied. It is read by [`crate::problem::parse`]
//! and rebuilt from the six fields that reader validated, so a `private` or a
//! `scratch` member cannot reach a prompt even by accident -- there is no code
//! path along which it could travel.
//!
//! # An interrupted call is not lost work
//!
//! The file named by `--state` holds the goal and one entry per attempt. A
//! call that could not be made, or that was interrupted, leaves it `pending`,
//! and running the same command again without `--resume` is refused rather
//! than started over: the caller is told there is saved work before anything
//! replaces it.

use crate::cli::{Args, Refused};
use crate::json::{obj, s, Json};
use crate::paths;
use crate::problem::{self, Problem};
use crate::tools;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The schema of the file `--state` names.
pub const STATE_SCHEMA: &str = "catalyst.ai-state.v1";
/// The command run when neither `--command` nor the variable names another.
pub const DEFAULT_COMMAND: &str = "claude";
/// The model asked for when neither `--model` nor the variable names another.
pub const DEFAULT_MODEL: &str = "claude-opus-5";
/// The variable that names the command, for a caller who cannot pass a flag.
pub const COMMAND_VARIABLE: &str = "CATALYST_AI_COMMAND";
/// The variable that names the model.
pub const MODEL_VARIABLE: &str = "CATALYST_AI_MODEL";
/// The variable that names the argument shape of the user's own CLI.
pub const ARGS_VARIABLE: &str = "CATALYST_AI_ARGS";

/// The argument template used when neither `--provider-args` nor
/// [`ARGS_VARIABLE`] names another. It is the shape of the `claude` CLI, and
/// the acceptance oracles compare the resolved list word for word.
pub const DEFAULT_ARGS: &str = "-p --model {model} --output-format json";

/// What `{model}` stands for in a template.
const MODEL_PLACEHOLDER: &str = "{model}";

/// The one precedence every configurable value here follows: what the user
/// wrote on this command line, then what they configured in their shell, then
/// the default. A variable that is there but blank is not a configuration.
///
/// Keeping it in one function is what lets the rule be tested without setting
/// a variable in this process: the three callers below differ only in which
/// flag and which variable they read.
fn resolved(flag: Option<&str>, variable: Option<String>, fallback: &str) -> String {
    flag.map(str::to_owned)
        .or_else(|| variable.filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| fallback.to_owned())
}

/// The template: the flag, then the variable, then the default.
fn args_template(args: &Args) -> String {
    resolved(
        args.value("provider-args"),
        std::env::var(ARGS_VARIABLE).ok(),
        DEFAULT_ARGS,
    )
}

/// The argument list a template names, with the model put in.
///
/// # Why a template rather than a fixed list
///
/// Because Catalyst is not bought from anybody. It has no account, holds no
/// credential and calls no provider: it runs a command-line program its user
/// has already signed into with whatever plan they pay for, and reads what
/// that program prints. A fixed argument list would quietly make that one
/// vendor's CLI, and everybody on another plan would need a wrapper script to
/// use a tool that was supposed to be theirs already.
///
/// A template is split on whitespace and every `{model}` in it is replaced.
/// A template that never mentions `{model}` is run *without* a model argument
/// -- not with an empty one -- because some signed-in CLIs take no model, and
/// handing such a CLI an empty string would be handing it a bad argument
/// rather than leaving one out.
pub fn arguments(template: &str, model: &str) -> Result<Vec<String>, Refused> {
    let list: Vec<String> = template
        .split_whitespace()
        .map(|word| word.replace(MODEL_PLACEHOLDER, model))
        .collect();
    if list.is_empty() {
        return Err(Refused::new(
            "catalyst.ai_arguments_invalid",
            "the provider argument template names no arguments at all, so there would be \
             nothing to run the command with",
            format!(
                "give the argument shape of the CLI you are signed into, for example \
                 `--provider-args \"{DEFAULT_ARGS}\"` or \
                 `--provider-args \"exec --model {MODEL_PLACEHOLDER} --json\"`; a template \
                 that names no {MODEL_PLACEHOLDER} is run without a model argument"
            ),
        ));
    }
    Ok(list)
}

/// `catalyst ai …`. Returns the one JSON object the command prints.
pub fn run(args: &Args) -> Result<String, Refused> {
    if args.has("status") {
        return status(args);
    }
    if args.has("propose") {
        return propose(args);
    }
    Err(Refused::usage(
        "`catalyst ai` needs a subcommand: `status`, or `propose --goal TEXT --out FILE --state FILE`",
    ))
}

/// The configured command name: the flag, then the variable, then `claude`.
fn command_name(args: &Args) -> String {
    resolved(
        args.value("command"),
        std::env::var(COMMAND_VARIABLE).ok(),
        DEFAULT_COMMAND,
    )
}

/// The configured model name: the flag, then the variable, then the default.
fn model_name(args: &Args) -> String {
    resolved(
        args.value("model"),
        std::env::var(MODEL_VARIABLE).ok(),
        DEFAULT_MODEL,
    )
}

/// Where the configured command is, if it is anywhere.
///
/// A name with a separator in it is a place, and is used as written. A bare
/// name is looked for along `PATH`, in the order `PATH` gives. Nothing is run
/// to find out; a file that exists is the whole test, because running a
/// candidate to see whether it works is how a status check becomes a side
/// effect.
fn locate(name: &str) -> Option<PathBuf> {
    if name.contains(std::path::MAIN_SEPARATOR) || name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    let listed = std::env::var_os("PATH")?;
    std::env::split_paths(&listed)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// `catalyst ai status`. `ok:true` and exit 0 whether or not the command is
/// there, because a feature being unavailable is an answer and not a failure.
///
/// It names the exact argument list the command would be run with, so a person
/// on any plan can see what Catalyst is about to do before it does it, rather
/// than reading this source to find out.
fn status(args: &Args) -> Result<String, Refused> {
    let command = command_name(args);
    let model = model_name(args);
    let arguments = arguments(&args_template(args), &model)?;
    let available = locate(&command).is_some();
    let shown = arguments.join(" ");
    let (detail, remedy) = if available {
        (
            format!(
                "the command `{}` was found and will be run with the arguments `{}`, with the \
                 prompt on standard input and no shell",
                safe(&command),
                safe(&shown)
            ),
            "run `catalyst ai propose --goal TEXT --out FILE --state FILE` to ask it for a problem"
                .to_owned(),
        )
    } else {
        (
            format!(
                "the command `{}` is not on the search path, so the AI feature is unavailable",
                safe(&command)
            ),
            format!(
                "sign in to the command-line tool of whatever plan you have and put it on the \
                 path, or name another with `--command PATH` or the {COMMAND_VARIABLE} \
                 variable, and give its argument shape with `--provider-args` or \
                 {ARGS_VARIABLE}. every other command -- eval, export go, tui, tools -- works \
                 without it"
            ),
        )
    };
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("available", Json::Bool(available)),
        ("command", s(&command)),
        ("model", s(&model)),
        (
            "arguments",
            Json::Arr(arguments.iter().map(|word| s(word)).collect()),
        ),
        ("detail", s(&detail)),
        ("remedy", s(&remedy)),
    ])
    .render())
}

/// The saved work of one goal, and the file it lives in.
struct State {
    path: PathBuf,
    named: String,
    goal: String,
    attempts: Vec<Json>,
}

impl State {
    /// Record one attempt and write the file, still pending.
    ///
    /// A write that fails replaces the refusal with its own: a caller told to
    /// resume from a file that was never written would be told to do something
    /// that cannot work.
    fn stop(&mut self, outcome: &str, detail: String, code: &str, remedy: String) -> Refused {
        self.attempts
            .push(attempt(self.attempts.len() + 1, outcome, &detail));
        if let Err(refused) = self.write(true, None) {
            return refused;
        }
        Refused::new(code, detail, remedy)
    }

    fn write(&self, pending: bool, accepted: Option<Json>) -> Result<(), Refused> {
        let mut members = vec![
            ("schema", s(STATE_SCHEMA)),
            ("goal", s(&self.goal)),
            ("pending", Json::Bool(pending)),
            ("attempts", Json::Arr(self.attempts.clone())),
        ];
        if let Some(problem) = accepted {
            members.push(("accepted_problem", problem));
        }
        write_text(&self.path, &self.named, &obj(members).render_pretty())
    }
}

fn attempt(n: usize, outcome: &str, detail: &str) -> Json {
    obj(vec![
        ("n", Json::Num(n as f64)),
        ("outcome", s(outcome)),
        ("detail", s(detail)),
    ])
}

fn write_text(path: &std::path::Path, named: &str, text: &str) -> Result<(), Refused> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, text).map_err(|error| {
        Refused::new(
            "catalyst.path_refused",
            format!(
                "`{named}` could not be written: {}",
                format!("{:?}", error.kind()).to_lowercase()
            ),
            "name a file inside the current directory that this user may write",
        )
    })
}

/// `catalyst ai propose …`.
fn propose(args: &Args) -> Result<String, Refused> {
    let goal = args.required("goal", "ai propose")?.to_owned();
    let out_named = args.required("out", "ai propose")?.to_owned();
    let state_named = args.required("state", "ai propose")?.to_owned();
    let resuming = args.has("resume");

    let out_path = paths::resolve(&out_named)?.full;
    let state_path = paths::resolve(&state_named)?.full;

    // A template that names no arguments is a mistake in the command line, not
    // an attempt that failed, so it is refused here rather than recorded as a
    // pending call somebody would be told to resume.
    let model = model_name(args);
    let argument_list = arguments(&args_template(args), &model)?;

    // Saved work is read before anything else happens, so a pending goal is
    // never replaced by a call the caller did not know they were repeating.
    let saved = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|text| Json::parse(&text).ok());
    let mut attempts: Vec<Json> = Vec::new();
    if let Some(saved) = &saved {
        let pending = saved
            .get("pending")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let saved_goal = saved.get("goal").and_then(Json::as_str).unwrap_or_default();
        if pending && !resuming {
            return Err(Refused::new(
                "catalyst.ai_pending",
                format!("`{}` holds a call that did not finish", safe(&state_named)),
                format!(
                    "run the same command again with --resume to continue that goal, or name \
                     another file with --state FILE to start a new one; the saved work in \
                     `{}` is left as it is",
                    safe(&state_named)
                ),
            ));
        }
        if pending && saved_goal != goal {
            return Err(Refused::new(
                "catalyst.ai_state_mismatch",
                format!(
                    "`{}` holds a pending call for a different goal",
                    safe(&state_named)
                ),
                "resume with the goal the state file was saved for, or name another file \
                 with --state FILE to start a new one",
            ));
        }
        if saved_goal == goal {
            if let Some(previous) = saved.get("attempts").and_then(Json::as_arr) {
                attempts = previous.to_vec();
            }
        }
    }

    let context = match args.value("context") {
        Some(named) => Some(problem::parse(&paths::read_input(named)?)?),
        None => None,
    };
    let prompt = prompt_for(&goal, context.as_ref());
    let command = command_name(args);
    let transcript = args.value("transcript").map(str::to_owned);

    let mut state = State {
        path: state_path,
        named: state_named,
        goal,
        attempts,
    };

    let Some(program) = locate(&command) else {
        return Err(state.stop(
            "ai_unavailable",
            format!(
                "the command `{}` is not on the search path, so no call could be made",
                safe(&command)
            ),
            "catalyst.ai_unavailable",
            "install that command, or name another with `--command PATH`, then run again with \
             --resume. `catalyst ai status` reports what is configured, and the manual \
             commands work without it"
                .to_owned(),
        ));
    };

    let reply = match call_command(&program, &argument_list, &prompt) {
        Ok(reply) => reply,
        Err(detail) => {
            record_transcript(transcript.as_deref(), &prompt, "")?;
            return Err(state.stop(
                "ai_call_failed",
                detail,
                "catalyst.ai_call_failed",
                "run the same command again with --resume to try the saved goal once more"
                    .to_owned(),
            ));
        }
    };
    record_transcript(transcript.as_deref(), &prompt, &reply)?;

    let proposal = match read_proposal(&reply) {
        Ok(proposal) => proposal,
        Err(detail) => {
            return Err(state.stop(
                "proposal_refused",
                detail,
                "catalyst.proposal_refused",
                "ask again with a clearer goal, or write the problem by hand; the answer must \
                 be one catalyst.problem.v1 object with an input and a domain for every \
                 parameter of its function"
                    .to_owned(),
            ));
        }
    };

    let document = proposal.to_json();
    write_text(&out_path, &out_named, &document.render_pretty())?;
    state.attempts.push(attempt(
        state.attempts.len() + 1,
        "accepted",
        "the answer was one valid catalyst.problem.v1 object",
    ));
    state.write(false, Some(document.clone()))?;
    Ok(obj(vec![
        ("ok", Json::Bool(true)),
        ("out", s(&out_named)),
        ("validated", Json::Bool(true)),
        ("proposal", document),
    ])
    .render())
}

/// Run the command and hand back its standard output, or say what went wrong.
///
/// The argument list has already been resolved, the prompt goes on standard
/// input, and no shell is involved: `Command` takes the program and its
/// arguments as separate values, so nothing in the prompt, in the goal or in
/// the template can become a word the shell would re-read. The environment is
/// inherited untouched, which is how the CLI finds the session its user
/// already signed in with; Catalyst neither reads that nor adds to it. The
/// child's standard error is captured and dropped rather than inherited --
/// whatever it prints is not Catalyst's to put on somebody's terminal, and a
/// signed-in CLI is exactly the sort of program that prints account details
/// there.
fn call_command(
    program: &std::path::Path,
    arguments: &[String],
    prompt: &str,
) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "the AI command could not be started: {}",
                format!("{:?}", error.kind()).to_lowercase()
            )
        })?;
    if let Some(mut sink) = child.stdin.take() {
        // A command that will not read the prompt is a command that failed;
        // the closed pipe is reported by the exit status below.
        let _ = sink.write_all(prompt.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "the AI command could not be waited for".to_owned())?;
    if !output.status.success() {
        return Err(match output.status.code() {
            Some(code) => format!("the AI command ended with exit status {code}"),
            None => "the AI command was interrupted before it finished".to_owned(),
        });
    }
    String::from_utf8(output.stdout)
        .map_err(|_| "the AI command's output is not valid UTF-8 text".to_owned())
}

/// The prompt, built from approved data only.
fn prompt_for(goal: &str, context: Option<&Problem>) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are answering a request from the `catalyst` command-line tool. Catalyst turns a \
         numerical goal into a checked, self-contained export.\n\n",
    );
    prompt.push_str("The goal, exactly as the operator wrote it:\n\n");
    prompt.push_str(goal);
    prompt.push_str("\n\nThe tool surface Catalyst offers, as its own discovery document:\n\n");
    prompt.push_str(&tools::discovery().render());
    if let Some(problem) = context {
        prompt.push_str(
            "\n\nAn existing problem, given as context. These are its validated fields, and \
             they are all that was sent:\n\n",
        );
        prompt.push_str(&problem.to_json().render());
    }
    prompt.push_str(
        "\n\nAnswer with exactly one JSON object of schema \"catalyst.problem.v1\": string \
         `schema`, `name`, `goal` and `function`, an `inputs` object of finite numbers, and a \
         `domains` object mapping each parameter to {\"min\", \"max\", \"unit\"} with min below \
         max and the input inside that range. `function` is written as \
         `func name(a, b) = <expression>`, using only + - * / and the calls exp, log, sin, cos, \
         sqrt, abs and pow. Every parameter of the function needs an entry in `inputs` and in \
         `domains`, and nothing that is not a parameter may appear in either. Do not add any \
         other member, and do not explain the answer.\n",
    );
    prompt
}

/// The transcript of §5: the exact prompt sent and the raw output received.
fn record_transcript(named: Option<&str>, prompt: &str, received: &str) -> Result<(), Refused> {
    let Some(named) = named else {
        return Ok(());
    };
    let path = paths::resolve(named)?.full;
    let mut text = String::from("--- prompt sent ---\n");
    text.push_str(prompt);
    text.push_str("\n--- output received ---\n");
    text.push_str(received);
    text.push('\n');
    write_text(&path, named, &text)
}

/// Read the answer text out of whatever the command printed, then the first
/// JSON object in it, then validate that object as a problem.
fn read_proposal(reply: &str) -> Result<Problem, String> {
    let text = answer_text(reply);
    let Some(candidate) = first_object(&text) else {
        return Err("the answer contains no JSON object".to_owned());
    };
    problem::parse(candidate).map_err(|refused| {
        format!(
            "the answer is not a valid catalyst.problem.v1 object ({}): {}",
            refused.code, refused.detail
        )
    })
}

/// What the command actually said, whichever way it says things.
///
/// Some CLIs wrap their answer in an envelope -- `{"type":"result","result":
/// "<text>",…}` -- and some simply print it. Requiring the envelope would mean
/// only the CLIs that happen to use it could be used at all, which is the
/// opposite of the point: the user's plan decides which program runs here, and
/// their program's output shape is not theirs to change.
///
/// The envelope is recognised, rather than assumed, by two things at once: a
/// string `result` member, and no `schema` member. The second half matters
/// because a CLI that prints a bare `catalyst.problem.v1` object is printing
/// the answer itself, and a problem that happened to carry a `result` field
/// must not be mistaken for a wrapper around one.
///
/// The text is returned owned because an envelope's `result` is a JSON string:
/// the answer inside it is escaped, and what the caller needs is the
/// *unescaped* text the model wrote. Handing back a slice of the raw output
/// would hand back the escaping with it, and the braces of the object would no
/// longer match the quotes around them.
fn answer_text(reply: &str) -> String {
    if let Ok(document) = Json::parse(reply.trim()) {
        if document.get("schema").is_none() {
            if let Some(text) = document.get("result").and_then(Json::as_str) {
                return text.to_owned();
            }
        }
    }
    reply.to_owned()
}

/// The first balanced `{…}` in a piece of text, fences and prose around it
/// allowed. Braces inside strings are counted as text, which is what makes a
/// function body containing one safe to read.
fn first_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return text.get(start..=i);
                }
            }
            _ => {}
        }
    }
    None
}

/// A configured name, in a form a message can carry.
fn safe(name: &str) -> String {
    let mut out: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    if name.chars().count() > 64 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Problem {
        problem::parse(
            "{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
             \"private\":\"UNAPPROVED\",\"function\":\"func p(x) = x * x\",\
             \"inputs\":{\"x\":3},\"domains\":{\"x\":{\"min\":0,\"max\":9,\"unit\":\"m\"}}}",
        )
        .expect("valid")
    }

    #[test]
    fn the_prompt_carries_the_goal_the_context_and_the_instruction() {
        let prompt = prompt_for("settle quickly", Some(&context()));
        assert!(prompt.contains("settle quickly"));
        assert!(prompt.contains("func p(x) = x * x"));
        assert!(prompt.contains("catalyst.problem.v1"));
    }

    #[test]
    fn no_unapproved_member_of_the_context_reaches_the_prompt() {
        let prompt = prompt_for("a goal", Some(&context()));
        assert!(!prompt.contains("UNAPPROVED"), "{prompt}");
        assert!(!prompt.contains("private"), "{prompt}");
    }

    /// The command line wins over the shell, the shell wins over the default,
    /// and a variable that is set but blank configures nothing. The same rule
    /// governs the command, the model and the argument template.
    #[test]
    fn the_flag_beats_the_environment_and_the_environment_beats_the_default() {
        assert_eq!(
            resolved(Some("flag-wins"), Some("env-loses".to_owned()), "default"),
            "flag-wins"
        );
        assert_eq!(
            resolved(None, Some("from-the-shell".to_owned()), "default"),
            "from-the-shell"
        );
        assert_eq!(resolved(None, None, "default"), "default");
        assert_eq!(resolved(None, Some("   ".to_owned()), "default"), "default");
        assert_eq!(resolved(None, Some(String::new()), "default"), "default");
    }

    /// The shape every earlier oracle was bound to. Making Catalyst work with
    /// any plan must not move the list it already used.
    #[test]
    fn the_default_template_resolves_to_exactly_the_agreed_argument_list() {
        assert_eq!(
            arguments(DEFAULT_ARGS, DEFAULT_MODEL).expect("resolved"),
            vec!["-p", "--model", "claude-opus-5", "--output-format", "json"]
        );
    }

    #[test]
    fn another_plans_cli_shape_is_expressible_without_touching_this_source() {
        assert_eq!(
            arguments("exec --model {model} --json", "a-plan-model").expect("resolved"),
            vec!["exec", "--model", "a-plan-model", "--json"]
        );
        assert_eq!(
            arguments("  run   --quiet\t--as {model}  ", "m").expect("resolved"),
            vec!["run", "--quiet", "--as", "m"]
        );
        assert_eq!(
            arguments("--model={model}", "m").expect("resolved"),
            vec!["--model=m"]
        );
    }

    /// Naming no model means no model argument, not an empty one: a CLI handed
    /// `""` has been handed a bad argument, not spared a good one.
    #[test]
    fn a_template_without_a_model_runs_without_one() {
        assert_eq!(
            arguments("--prompt-stdin", "unused").expect("resolved"),
            vec!["--prompt-stdin"]
        );
    }

    #[test]
    fn a_template_that_names_nothing_is_refused_with_a_worked_example() {
        for template in ["", "   ", "\t\n "] {
            let refused = arguments(template, "m").expect_err("refused");
            assert_eq!(refused.code, "catalyst.ai_arguments_invalid");
            assert!(refused.detail.len() >= 8);
            assert!(
                refused.remedy.contains("--provider-args"),
                "{}",
                refused.remedy
            );
        }
    }

    /// A CLI that simply prints its answer is as usable as one that wraps it.
    #[test]
    fn an_answer_is_read_whether_or_not_it_arrives_in_an_envelope() {
        let problem = context().to_json().render();
        let plain = format!("Here you go:\n\n```json\n{problem}\n```\n");
        let wrapped = obj(vec![("type", s("result")), ("result", s(&plain))]).render();
        assert_eq!(answer_text(&plain), plain);
        assert_eq!(answer_text(&wrapped), plain);
        assert_eq!(
            read_proposal(&plain).expect("plain").function,
            "func p(x) = x * x"
        );
        assert_eq!(
            read_proposal(&wrapped).expect("wrapped").function,
            "func p(x) = x * x"
        );
    }

    /// A CLI that prints the problem object and nothing else is printing the
    /// answer, not an envelope around one.
    #[test]
    fn a_bare_problem_object_is_the_answer_rather_than_a_wrapper() {
        let problem = context().to_json().render();
        assert_eq!(answer_text(&problem), problem);
        assert!(read_proposal(&problem).is_ok());
    }

    #[test]
    fn the_answer_is_read_out_of_prose_and_fences() {
        let problem = context().to_json().render();
        let reply = obj(vec![
            ("type", s("result")),
            (
                "result",
                s(&format!("Here you go:\n\n```json\n{problem}\n```\n")),
            ),
        ])
        .render();
        let read = read_proposal(&reply).expect("read");
        assert_eq!(read.function, "func p(x) = x * x");
    }

    #[test]
    fn a_brace_inside_a_string_does_not_end_the_object() {
        assert_eq!(first_object("x {\"a\":\"}\"} y"), Some("{\"a\":\"}\"}"));
    }

    #[test]
    fn an_answer_that_is_not_a_problem_is_refused_rather_than_written() {
        let reply = obj(vec![("type", s("result")), ("result", s("no idea, sorry"))]).render();
        assert!(read_proposal(&reply).is_err());
    }
}
