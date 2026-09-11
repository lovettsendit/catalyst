//! One call: source in, value and gradient out.
//!
//! # Who this is for
//!
//! A caller that has a function and wants its derivative, and does not want to
//! know that there is an IR, an optimiser, a type analysis, an activity
//! analysis and two transforms between those two facts. That describes a
//! scientist, and it describes an agent, and the interface that suits both is
//! the same one.
//!
//! ```text
//!     let answer = catalyst::api::gradient("func f(x, y) = x*y + sin(x)", &[0.7, 1.3])?;
//!     answer.value        // f(0.7, 1.3)
//!     answer.gradient     // [df/dx, df/dy]
//!     answer.names        // ["x", "y"]
//! ```
//!
//! # Why the gradient comes back with its names attached
//!
//! A bare `[f64; 2]` makes the caller responsible for remembering which
//! parameter was which, and that is precisely the bookkeeping that goes wrong
//! silently. The names came in with the source; sending them back costs
//! nothing and removes a whole class of mistake.
//!
//! # `solve`: the same thing, without Rust
//!
//! [`solve`] takes JSON and returns JSON, so a caller that is a process rather
//! than a crate can use Catalyst over a pipe. Success and refusal are both
//! objects with a stable shape, and a refusal carries the same code, detail and
//! remedy any other refusal in this crate does. An agent that can read one can
//! read all of them.
//!
//! The optimiser runs before differentiation, because that is the whole thesis
//! of the project and it would be strange for the front door to opt out of it.

use crate::activity::Activity;
use crate::forward::Job;
use crate::interp::{Interp, Memory};
use crate::opt::{optimise, Level};
use crate::refuse::{Cause, Refusal};
use crate::reverse::revdiff;
use crate::text;

/// Maximum UTF-8 request size accepted by [`solve`], in bytes.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// A value and its gradient, with the names they belong to.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub value: f64,
    pub gradient: Vec<f64>,
    pub names: Vec<String>,
    /// Instructions the primal has after optimisation, and instructions the
    /// reverse sweep added. Reported rather than hidden because "what did the
    /// gradient cost" is the first question anyone benchmarking asks, and the
    /// honest answer is already known here.
    pub primal_insts: usize,
    pub adjoint_insts: usize,
}

impl Answer {
    /// The answer as a JSON object. Hand-written, like every other renderer in
    /// this crate, because there are no dependencies to reach for.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\"ok\":true,\"value\":");
        push_f64(self.value, &mut s);
        s.push_str(",\"gradient\":{");
        for (i, (name, g)) in self.names.iter().zip(&self.gradient).enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('"');
            push_escaped(name, &mut s);
            s.push_str("\":");
            push_f64(*g, &mut s);
        }
        s.push_str("},\"cost\":{\"primal_insts\":");
        s.push_str(&self.primal_insts.to_string());
        s.push_str(",\"adjoint_insts\":");
        s.push_str(&self.adjoint_insts.to_string());
        s.push_str("}}");
        s
    }
}

/// Differentiate `source` at `at`, with respect to every parameter.
///
/// Reverse mode, so the cost is one pass whatever the number of inputs. The
/// language [`crate::text`] accepts is straight-line by construction, so the
/// mode that would refuse control flow can never meet any here.
pub fn gradient(source: &str, at: &[f64]) -> Result<Answer, Refusal> {
    let mut parsed = text::parse(source)?;
    if at.len() != parsed.params.len() {
        return Err(Refusal::about_job(
            &parsed.name,
            Cause::WrongArity {
                name: parsed.name.clone(),
                expected: parsed.params.len(),
                got: at.len(),
            },
        ));
    }

    optimise(&mut parsed.module, parsed.id, Level::Full);
    let job = Job::new(vec![Activity::Active; parsed.params.len()]);
    let (d, stats) = revdiff(&mut parsed.module, parsed.id, &job)?;

    let mut mem = Memory::default();
    let out = mem.place(&vec![0.0; parsed.params.len()]);
    let mut args: Vec<u64> = at.iter().map(|x| x.to_bits()).collect();
    args.push(out);
    args.push(1.0f64.to_bits());

    let mut it = Interp::with_memory(&parsed.module, mem);
    let value = match it.call(d, &args) {
        Ok(bits) => f64::from_bits(bits),
        // A fault is a fact about the program, not about this crate: a division
        // that reached a bad pointer, or an argument count the interpreter
        // could not satisfy. It is reported in the same shape as everything
        // else rather than panicking under a caller that cannot catch it.
        Err(fault) => {
            return Err(Refusal::about_job(
                &parsed.name,
                Cause::NoDerivativeRule {
                    op: format!("the program faulted while running: {fault:?}"),
                },
            ))
        }
    };

    Ok(Answer {
        value,
        gradient: it.mem.read(out, parsed.params.len()),
        names: parsed.params,
        primal_insts: stats.primal_insts,
        adjoint_insts: stats.adjoint_insts,
    })
}

/// The same thing over JSON, for a caller that is not a Rust crate.
///
/// Input: `{"source": "func f(x) = x*x", "at": [3.0]}`.
/// Output: an [`Answer`] object, or `{"ok":false, ...}` carrying the refusal.
///
/// Requests must be one JSON object containing exactly `source` and `at`,
/// with finite numeric coordinates, and at most 1 MiB of UTF-8 input.
/// Malformed requests return a structured refusal rather than partially
/// interpreting the input or silently dropping a coordinate.
pub fn solve(request: &str) -> String {
    let (source, at) = match crate::request_json::parse(request) {
        Ok(parsed) => parsed,
        Err(offset) => return Refusal::about_job(
            "<request>",
            Cause::Syntax {
                at: offset,
                found: "an invalid JSON request".to_owned(),
                expected: "one object with source and a finite-number at array, no duplicate or extra fields, at most 1 MiB".to_owned(),
            },
        )
        .to_json_envelope(),
    };
    match gradient(&source, &at) {
        Ok(answer) => answer.to_json(),
        Err(refusal) => refusal.to_json_envelope(),
    }
}

/// `f64` in a form JSON can carry.
///
/// JSON has no infinity and no NaN, and a bare `inf` would make the document
/// unparseable for the caller. They are written as `null` instead, which every
/// parser accepts and which reads as "there is no number here" -- true, and
/// better than a document that cannot be read at all.
fn push_f64(x: f64, out: &mut String) {
    if x.is_finite() {
        out.push_str(&format!("{x:?}"));
    } else {
        out.push_str("null");
    }
}

fn push_escaped(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

impl Refusal {
    /// The refusal, wrapped so success and failure have the same outer shape:
    /// both are objects with an `ok` field, so a caller tests one thing.
    pub fn to_json_envelope(&self) -> String {
        let inner = self.to_json();
        // `to_json` already produced a complete object; splicing `ok` in at the
        // front keeps one renderer for the refusal itself.
        format!("{{\"ok\":false,{}", &inner[1..])
    }
}
