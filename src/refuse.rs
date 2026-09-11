//! Why a transform refused, in a form a person and a program can both read.
//!
//! # The one rule this module exists to enforce
//!
//! **A gradient that is wrong is worse than no gradient at all.** A missing
//! derivative stops a run and gets fixed. A silently wrong one trains a model
//! that never converges, or converges on nothing, and the search for the cause
//! starts everywhere except the differentiator. Enzyme's own failure mode here
//! is an LLVM diagnostic at best and a miscompile at worst; Catalyst's rule is
//! that every construct it cannot differentiate correctly is refused by name,
//! before any code is generated.
//!
//! So the transforms return `Result<_, Refusal>` rather than a function that
//! might be right. There is no "best effort" path, and no flag that turns one
//! on: an approximation nobody asked for is indistinguishable from a bug.
//!
//! # Why a refusal is a structured value and not a string
//!
//! The caller is as likely to be a program as a person -- a training loop
//! deciding whether to fall back, an agent repairing the program it just
//! emitted, a test asserting that a specific construct is still refused. A
//! formatted sentence serves none of them: it cannot be matched without
//! parsing prose, and prose is exactly the thing that gets reworded.
//!
//! Every refusal therefore carries:
//!
//! * a `code`, stable and machine-readable, which is the thing to match on;
//! * `where` it happened, as a block label and an instruction index, so it can
//!   be pointed at rather than searched for;
//! * `what` was asked for, in the IR's own vocabulary;
//! * a `remedy`, because a refusal that does not say what to do instead is
//!   only half an answer.
//!
//! [`Refusal::to_json`] renders all of that. It is written by hand, an object
//! at a time, because this crate has no dependencies and adding one to print
//! six fields would be a poor trade.

use crate::ir::{BlockId, Value};
use std::fmt;

/// What was refused. One variant per reason, never a catch-all.
///
/// A catch-all is where the honesty leaks out: it collects the cases nobody
/// wrote a variant for, and then every one of them reaches the caller wearing
/// the same name. Adding a reason here is a deliberate act, and the compiler
/// makes it one by refusing an unmatched arm in [`Cause::code`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Cause {
    /// The control flow graph has a cycle. Reverse mode needs the block trace
    /// and the trip counts to replay it, and this build does not record them.
    CyclicControlFlow { block: String },
    /// The function branches. Reverse mode walks the program backwards, and
    /// which side of a branch ran is not recoverable without a recorded trace.
    /// Forward mode has no such problem: it follows the primal's own control
    /// flow, one tangent per value, so it is the honest answer here.
    BranchingControlFlow { blocks: usize },
    /// An operation with no derivative rule in this build.
    NoDerivativeRule { op: String },
    /// A call into a function that has not itself been differentiated.
    /// Differentiating through it would mean assuming its derivative is zero,
    /// which is a wrong answer rather than a missing one.
    OpaqueCall { callee: String },
    /// The job asked for a derivative but marked nothing active, so the answer
    /// is zero everywhere. That is nearly always a mis-specified job rather
    /// than a genuine request, so it is refused instead of silently answered.
    NothingActive,
    /// A pointer parameter was marked active, or a scalar was marked
    /// duplicated. The annotation and the type disagree.
    AnnotationMismatch {
        param: u32,
        asked: String,
        is: String,
    },
    /// A load or store whose object type analysis could not resolve. Shadowing
    /// it would mean guessing whether the bytes carry a derivative.
    UnresolvedObject { behind: String },
    /// The source did not parse. Carries the byte offset, so a caller can point
    /// at the character rather than describe it.
    Syntax {
        at: usize,
        found: String,
        expected: String,
    },
    /// A name the source used that this build does not define.
    UnknownName { name: String },
    /// A known function called with the wrong number of arguments.
    WrongArity {
        name: String,
        expected: usize,
        got: usize,
    },
}

impl Cause {
    /// The stable identifier. This is the string a program matches on, so it
    /// is chosen once and then treated as part of the public interface: the
    /// prose in `Display` may be reworded freely, and these may not.
    pub fn code(&self) -> &'static str {
        match self {
            Cause::CyclicControlFlow { .. } => "catalyst.cyclic_control_flow",
            Cause::BranchingControlFlow { .. } => "catalyst.branching_control_flow",
            Cause::NoDerivativeRule { .. } => "catalyst.no_derivative_rule",
            Cause::OpaqueCall { .. } => "catalyst.opaque_call",
            Cause::NothingActive => "catalyst.nothing_active",
            Cause::AnnotationMismatch { .. } => "catalyst.annotation_mismatch",
            Cause::UnresolvedObject { .. } => "catalyst.unresolved_object",
            Cause::Syntax { .. } => "catalyst.syntax",
            Cause::UnknownName { .. } => "catalyst.unknown_name",
            Cause::WrongArity { .. } => "catalyst.wrong_arity",
        }
    }

    /// What to do instead. Every refusal has one; a refusal that only says no
    /// leaves the caller exactly where it was.
    pub fn remedy(&self) -> &'static str {
        match self {
            Cause::CyclicControlFlow { .. } => {
                "differentiate this function in forward mode, which needs no trace, \
                 or unroll the loop before differentiating it"
            }
            Cause::BranchingControlFlow { .. } => {
                "differentiate this function in forward mode, which follows the primal's \
                 own control flow, or express the branch as a Select so it becomes \
                 straight-line"
            }
            Cause::NoDerivativeRule { .. } => {
                "rewrite the expression in terms of operations that have rules, \
                 or register a custom rule for this one"
            }
            Cause::OpaqueCall { .. } => {
                "differentiate the callee first and call the differentiated form, \
                 or mark the call Const if its result genuinely carries no derivative"
            }
            Cause::NothingActive => "mark at least one parameter Active or Duplicated in the job",
            Cause::AnnotationMismatch { .. } => {
                "use Active for scalars and Duplicated for pointers"
            }
            Cause::UnresolvedObject { .. } => {
                "give the pointer parameter a type hint on the job, with Job::hint"
            }
            Cause::Syntax { .. } => {
                "the shape is `func name(a, b) = expression`, with + - * / ^, \
                 parentheses, and the named functions"
            }
            Cause::UnknownName { .. } => {
                "use a declared parameter, or one of the built-in functions \
                 listed by `catalyst::text::names`"
            }
            Cause::WrongArity { .. } => "check the argument count against `catalyst::text::names`",
        }
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::CyclicControlFlow { block } => write!(
                f,
                "the control flow graph has a cycle reaching block {block}, and reverse \
                 mode cannot replay it without a recorded trace"
            ),
            Cause::BranchingControlFlow { blocks } => write!(
                f,
                "the function has {blocks} blocks, and reverse mode in this build \
                 replays only straight-line code"
            ),
            Cause::NoDerivativeRule { op } => {
                write!(f, "no derivative rule for {op} in this build")
            }
            Cause::OpaqueCall { callee } => write!(
                f,
                "the call to {callee} has no differentiated form, and treating its \
                 derivative as zero would be a wrong answer rather than a missing one"
            ),
            Cause::NothingActive => write!(
                f,
                "no parameter was marked Active or Duplicated, so every derivative \
                 asked for is zero by construction"
            ),
            Cause::AnnotationMismatch { param, asked, is } => write!(
                f,
                "parameter {param} is annotated {asked} but its type is {is}"
            ),
            Cause::Syntax {
                at,
                found,
                expected,
            } => write!(f, "at byte {at}: expected {expected}, found {found}"),
            Cause::UnknownName { name } => {
                write!(f, "{name} is not a parameter or a known function")
            }
            Cause::WrongArity {
                name,
                expected,
                got,
            } => write!(f, "{name} takes {expected} argument(s), given {got}"),
            Cause::UnresolvedObject { behind } => write!(
                f,
                "type analysis resolved the object behind this pointer only as {behind}, \
                 so whether its bytes carry a derivative is unknown"
            ),
        }
    }
}

/// A refusal: the cause, and exactly where it was hit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refusal {
    /// Boxed, so a `Result` is sized by its success value rather than by an
    /// explanation the happy path never carries. `Cause` holds the strings, and
    /// inline it made every `Result<_, Refusal>` in the crate 128 bytes wide --
    /// the cost of an error nobody had yet, paid on every call that succeeded.
    /// `Deref` means `refusal.cause.code()` still reads the same.
    pub cause: Box<Cause>,
    /// The block the transform was in. `None` when the refusal is about the
    /// job rather than about a place in the program.
    pub block: Option<BlockId>,
    /// The label of that block, carried so a reader does not have to hold the
    /// function to make sense of the number.
    pub block_label: Option<String>,
    /// The instruction at fault, where there is one.
    pub value: Option<Value>,
    /// The function the transform was asked to differentiate.
    pub function: String,
}

impl Refusal {
    /// A refusal about the job as a whole, with no place in the program.
    pub fn about_job(function: &str, cause: Cause) -> Self {
        Refusal {
            cause: Box::new(cause),
            block: None,
            block_label: None,
            value: None,
            function: function.to_owned(),
        }
    }

    /// A refusal about one instruction.
    pub fn at(function: &str, block: BlockId, label: &str, value: Value, cause: Cause) -> Self {
        Refusal {
            cause: Box::new(cause),
            block: Some(block),
            block_label: Some(label.to_owned()),
            value: Some(value),
            function: function.to_owned(),
        }
    }

    /// The stable code, for matching.
    pub fn code(&self) -> &'static str {
        self.cause.code()
    }

    /// The refusal as a JSON object, for a caller that is a program.
    ///
    /// Hand-written, because the crate has no dependencies and this is six
    /// fields. Only `function`, the labels and the prose can contain anything
    /// needing an escape, and [`escape`] handles the whole of what JSON
    /// requires: the two structural characters, the five short escapes, and
    /// every remaining control character as `\u00XX`.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\"code\":\"");
        s.push_str(self.code());
        s.push_str("\",\"function\":\"");
        escape(&self.function, &mut s);
        s.push_str("\",\"detail\":\"");
        escape(&self.cause.to_string(), &mut s);
        s.push_str("\",\"remedy\":\"");
        escape(self.cause.remedy(), &mut s);
        s.push_str("\",\"block\":");
        match (&self.block, &self.block_label) {
            (Some(b), Some(label)) => {
                s.push_str("{\"id\":");
                s.push_str(&b.to_string());
                s.push_str(",\"label\":\"");
                escape(label, &mut s);
                s.push_str("\"}");
            }
            _ => s.push_str("null"),
        }
        s.push_str(",\"value\":");
        match self.value {
            Some(v) => s.push_str(&v.to_string()),
            None => s.push_str("null"),
        }
        s.push('}');
        s
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: refused in {}", self.code(), self.function)?;
        if let Some(label) = &self.block_label {
            write!(f, ", block {label}")?;
        }
        if let Some(v) = self.value {
            write!(f, ", instruction %{v}")?;
        }
        write!(f, " -- {}. Instead: {}", self.cause, self.cause.remedy())
    }
}

/// Escape one string into a JSON string body.
///
/// The control characters matter more here than they look: a block label or a
/// function name reaches this crate from whoever built the IR, and a caller
/// parsing the output must not have to defend against what they chose to put
/// in it.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    let nibble = ((c as u32) >> shift) & 0xf;
                    out.push(char::from_digit(nibble, 16).expect("a nibble is a hex digit"));
                }
            }
            c => out.push(c),
        }
    }
}
