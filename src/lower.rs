//! The intermediate form every export is generated from.
//!
//! `docs/interface.md` §3 and §9. Catalyst exports the same problem to Go and
//! to R, and the two have to agree about arithmetic to the last bits. The only
//! way to promise that is for both to be printings of *one* instruction list,
//! so the list is produced here and each backend does nothing but render it.
//!
//! # Why the list is the differentiated, optimised IR
//!
//! Because the number an export must reproduce is the number the engine
//! computes, and the engine does not compute the source text. It parses it,
//! optimises it, and differentiates the *optimised* form -- that reordering is
//! the whole thesis of the project, and it moves the last bits of every
//! result. A backend that re-read the expression and emitted a textbook
//! derivative would agree to about eight digits and disagree in a way nobody
//! could explain, and a *second* such backend would disagree with the first
//! somewhere else again.
//!
//! So [`lower`] runs exactly what [`crate::api::gradient`] runs, and
//! [`straight_line`] picks out the instructions that matter, in order. Two
//! renderings of that list are two spellings of one computation.

use crate::activity::Activity;
use crate::cli::Refused;
use crate::forward::Job;
use crate::ir::{Func, FuncId, Module, Term, Value};
use crate::opt::{optimise, Level};
use crate::problem::Problem;
use crate::reverse::revdiff;
use crate::text;

/// A problem, parsed, optimised and differentiated: the form both exports
/// print.
pub struct Lowered {
    pub module: Module,
    /// The differentiated function inside it.
    pub id: FuncId,
    /// How many parameters the original function declared. Parameters past
    /// this are the adjoint's own: the gradient buffer and the seed.
    pub arity: usize,
}

impl Lowered {
    pub fn func(&self) -> &Func {
        self.module.get(self.id)
    }
}

/// Parse, optimise and differentiate, in the order [`crate::api::gradient`]
/// does it.
pub fn lower(problem: &Problem) -> Result<Lowered, Refused> {
    // An LLVM computation has no expression to lower: the exports refuse it
    // by name rather than trying to read a function name as one.
    if let Some(refused) = problem.portable_export_unavailable() {
        return Err(refused);
    }
    let mut parsed = text::parse(&problem.function).map_err(Refused::from)?;
    let arity = parsed.params.len();
    optimise(&mut parsed.module, parsed.id, Level::Full);
    let job = Job::new(vec![Activity::Active; arity]);
    let (id, _stats) = revdiff(&mut parsed.module, parsed.id, &job).map_err(Refused::from)?;
    Ok(Lowered {
        module: parsed.module,
        id,
        arity,
    })
}

/// The instructions an export has to emit, in order, and the value it
/// returns.
///
/// # Why dead instructions are dropped
///
/// Not as an optimisation. Go refuses to compile a declared variable nobody
/// reads, so an adjoint the sweep produced and never used would be a build
/// error rather than a wasted multiply. R would accept it, but an export that
/// carried statements with no effect would be asking its reader to work out
/// which lines matter. A backwards liveness walk keeps every store and
/// everything the returned value depends on, and nothing else.
pub fn straight_line(f: &Func) -> Result<(Vec<Value>, Option<Value>), Refused> {
    if f.blocks.len() != 1 {
        return Err(unsupported("a function with more than one block"));
    }
    let returned = match f.blocks[0].term {
        Term::Ret(Some(v)) => Some(v),
        _ => None,
    };

    let mut live = vec![false; f.insts.len()];
    if let Some(r) = returned {
        live[r as usize] = true;
    }
    let mut keep: Vec<Value> = Vec::new();
    for &v in f.blocks[0].insts.iter().rev() {
        if !live[v as usize] && !f.has_effect(v) {
            continue;
        }
        keep.push(v);
        for operand in f.operands(v) {
            live[operand as usize] = true;
        }
    }
    keep.reverse();
    Ok((keep, returned))
}

/// The refusal both backends give for a construct they cannot print. The
/// remedy is the same either way: the expression language they generate from
/// is straight-line, and something outside it arrived.
pub fn unsupported(what: &str) -> Refused {
    Refused::new(
        "catalyst.not_implemented",
        format!("this build cannot export {what}"),
        "express the function in the straight-line expression language, \
         which is what `catalyst export` generates from",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(function: &str) -> Problem {
        crate::problem::parse(&format!(
            "{{\"schema\":\"catalyst.problem.v1\",\"name\":\"p\",\"goal\":\"g\",\
             \"function\":{},\"inputs\":{{\"x\":2}},\
             \"domains\":{{\"x\":{{\"min\":0,\"max\":9,\"unit\":\"\"}}}}}}",
            crate::json::s(function).render()
        ))
        .expect("valid")
    }

    /// Both exports render this one list, so it has to be the same list every
    /// time the same problem is lowered.
    #[test]
    fn one_problem_lowers_to_one_instruction_list() {
        let p = problem("func p(x) = x * x + sin(x)");
        let first = lower(&p).expect("lowered");
        let second = lower(&p).expect("lowered");
        let (keep_a, ret_a) = straight_line(first.func()).expect("straight line");
        let (keep_b, ret_b) = straight_line(second.func()).expect("straight line");
        assert_eq!(keep_a, keep_b);
        assert_eq!(ret_a, ret_b);
        assert!(!keep_a.is_empty());
    }

    #[test]
    fn the_lowered_function_carries_the_adjoints_own_parameters() {
        let p = problem("func p(x) = x * x");
        let lowered = lower(&p).expect("lowered");
        assert_eq!(lowered.arity, 1);
        // The gradient buffer and the seed come after the declared ones.
        assert!(lowered.func().insts.len() > 1);
    }
}
