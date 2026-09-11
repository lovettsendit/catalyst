//! Failure rehearsal: `catalyst rehearse …`.
//!
//! `docs/interface.md` §8. The whole outcome is one argument: before believing
//! that a service survives something, make it happen, on purpose, somewhere it
//! costs nothing.
//!
//! ```text
//!   standin      write a stand-in service to aim at
//!   synth-trace  make traffic nobody recorded from anybody
//!   import       reduce a trace to shapes, keeping none of its content
//!   init/status  record the target, the criteria and the disturbances
//!   replay       drive the traffic under one disturbance, and judge it
//!   reduce       turn the failure into the smallest thing that reproduces it
//!   held-out     traffic no repair was tuned against
//!   compare      replay both against a baseline and a candidate
//!   report       say what improved, what did not, and what was not measured
//! ```
//!
//! # Everything is aimed at a stand-in, and the aim is checked first
//!
//! [`target`] decides what a rehearsal may drive, by reading the text, before
//! anything is opened. That check is repeated every time the configuration is
//! read, because a file can be edited between two commands. A rehearsal that
//! could be pointed at a payment service by changing one line of JSON would be
//! a tool for sending real orders, and no amount of care afterwards would fix
//! that.
//!
//! # Nothing that comes back is trusted with anything
//!
//! A trace is data ([`trace`]), an answer from the stand-in is a status code
//! and a length ([`wire`]), and neither can change a criterion, a target or a
//! path. The one thing read out of a body is an identifier to put in the next
//! URL, and it is bounded and templated before it goes anywhere.

pub mod compare;
pub mod config;
pub mod digest;
pub mod replay;
pub mod standin;
pub mod target;
pub mod trace;
pub mod wire;

use crate::cli::{Args, Refused};

/// The subcommands, in the order the usage text lists them.
pub const SUBCOMMANDS: &[&str] = &[
    "standin",
    "synth-trace",
    "import",
    "init",
    "status",
    "replay",
    "reduce",
    "held-out",
    "compare",
    "report",
];

/// `catalyst rehearse …`. Returns the one JSON object the command prints.
pub fn run(args: &Args) -> Result<String, Refused> {
    // A bare word, never a flag: `report --compare FILE` names `report`, and
    // `compare --held-out FILE` names `compare`.
    match SUBCOMMANDS.iter().find(|name| args.bare(name)) {
        Some(&"standin") => standin::write_standin(args),
        Some(&"synth-trace") => trace::synthesise(args),
        Some(&"import") => trace::import(args),
        Some(&"init") => config::init(args),
        Some(&"status") => config::status(args),
        Some(&"replay") => replay::replay(args),
        Some(&"reduce") => compare::reduce(args),
        Some(&"held-out") => compare::held_out(args),
        Some(&"compare") => compare::compare(args),
        Some(&"report") => compare::report(args),
        _ => Err(Refused::usage(format!(
            "`catalyst rehearse` needs a subcommand: {}",
            SUBCOMMANDS.join(", ")
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every subcommand this module dispatches is named in the usage text, and
    /// every one the usage text names is dispatched. A command a person can
    /// read about but not run, or run but not read about, is a defect either
    /// way.
    #[test]
    fn the_usage_text_and_the_dispatcher_name_the_same_subcommands() {
        let usage = crate::cli::usage("rehearse");
        for name in SUBCOMMANDS {
            assert!(
                usage.contains(&format!("catalyst rehearse {name}")),
                "`{name}` is dispatched but not in the usage text"
            );
        }
    }
}
