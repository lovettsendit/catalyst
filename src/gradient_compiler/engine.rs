//! The seam between the front ends and the engine: one function in the
//! engine's IR, differentiated in one mode, and executed only by the
//! engine's interpreter.
//!
//! # Why the interpreter, and why with a budget
//!
//! The program under differentiation came from somebody else's compiler.
//! Nothing here runs it as machine code: the interpreter has no instruction
//! that can reach the operating system, so the most a hostile module can do
//! is loop, and the fuel makes a loop a refusal rather than a hang. 2^26
//! instructions is a few seconds of interpretation, which is long enough for
//! any function this phase differentiates and short enough that a program
//! that never terminates for the given inputs is reported as such while the
//! operator is still watching.

use crate::activity::Activity;
use crate::forward::{fwddiff, Job};
use crate::interp::{Fault, Interp, Memory};
use crate::ir::{FuncId, Module, Ty};
use crate::refuse::Refusal;
use crate::reverse::revdiff;
use std::collections::HashMap;

/// The instruction budget of one evaluation.
pub const FUEL: u64 = 1 << 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Forward,
    Reverse,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Forward => "forward",
            Mode::Reverse => "reverse",
        }
    }
    pub fn read(word: &str) -> Option<Mode> {
        match word {
            "forward" => Some(Mode::Forward),
            "reverse" => Some(Mode::Reverse),
            _ => None,
        }
    }
}

/// A function and its derivative, ready to run.
pub struct Derivative {
    pub module: Module,
    /// The primal, after optimisation: what the verifier evaluates.
    pub primal: FuncId,
    /// The derivative function the transform produced.
    pub derivative: FuncId,
    pub mode: Mode,
    /// The primal's parameter types, in declaration order.
    pub param_tys: Vec<Ty>,
    /// The parameter index of each requested input, in request order.
    pub active: Vec<usize>,
    /// Instruction counts, for the `cost` a caller may ask about.
    pub primal_insts: usize,
    pub derived_insts: usize,
}

/// Differentiate `primal` in `module` with respect to `active`, in the mode
/// asked for, or in reverse mode when it applies and forward mode otherwise.
///
/// A forced mode the engine refuses is refused with the engine's own code;
/// nothing swaps it. When no mode is forced, only the two refusals that mean
/// "reverse mode does not cover this control flow" fall through to forward
/// mode; any other refusal is the answer.
pub fn differentiate(
    mut module: Module,
    primal: FuncId,
    active: Vec<usize>,
    rules: &HashMap<FuncId, FuncId>,
    mode: Option<Mode>,
) -> Result<Derivative, Refusal> {
    let param_tys = module.get(primal).params.clone();
    let primal_insts = module.get(primal).inst_count();
    let activities: Vec<Activity> = (0..param_tys.len())
        .map(|i| {
            if active.contains(&i) {
                Activity::Active
            } else {
                Activity::Const
            }
        })
        .collect();
    let mut job = Job::new(activities);
    for (&f, &g) in rules {
        job = job.rule(f, g);
    }

    let (chosen, id, derived_insts) = match mode {
        Some(Mode::Forward) => {
            let (id, stats) = fwddiff(&mut module, primal, &job);
            (Mode::Forward, id, stats.tangent_insts)
        }
        Some(Mode::Reverse) => {
            let (id, stats) = revdiff(&mut module, primal, &job)?;
            (Mode::Reverse, id, stats.adjoint_insts)
        }
        None => match revdiff(&mut module, primal, &job) {
            Ok((id, stats)) => (Mode::Reverse, id, stats.adjoint_insts),
            Err(refusal)
                if matches!(
                    refusal.code(),
                    "catalyst.cyclic_control_flow" | "catalyst.branching_control_flow"
                ) =>
            {
                let (id, stats) = fwddiff(&mut module, primal, &job);
                (Mode::Forward, id, stats.tangent_insts)
            }
            Err(refusal) => return Err(refusal),
        },
    };
    Ok(Derivative {
        module,
        primal,
        derivative: id,
        mode: chosen,
        param_tys,
        active,
        primal_insts,
        derived_insts,
    })
}

impl Derivative {
    /// The register word of one argument.
    fn word(ty: Ty, x: f64) -> u64 {
        match ty {
            Ty::Real => x.to_bits(),
            _ => x as i64 as u64,
        }
    }

    fn interpreter(&self) -> Interp<'_> {
        let mut it = Interp::new(&self.module);
        it.fuel = FUEL;
        it
    }

    /// The primal alone: the derivative transform nowhere in the path.
    pub fn primal_value(&self, at: &[f64]) -> Result<f64, Fault> {
        let words: Vec<u64> = self
            .param_tys
            .iter()
            .zip(at)
            .map(|(&ty, &x)| Self::word(ty, x))
            .collect();
        let mut it = self.interpreter();
        it.call(self.primal, &words).map(f64::from_bits)
    }

    /// The value and the requested partials at `at`, from the derivative
    /// function.
    pub fn evaluate(&self, at: &[f64]) -> Result<(f64, Vec<f64>), Fault> {
        match self.mode {
            Mode::Reverse => {
                let mut mem = Memory::default();
                let buffer = mem.place(&vec![0.0; self.param_tys.len()]);
                let mut words: Vec<u64> = self
                    .param_tys
                    .iter()
                    .zip(at)
                    .map(|(&ty, &x)| Self::word(ty, x))
                    .collect();
                words.push(buffer);
                words.push(1.0f64.to_bits());
                let mut it = Interp::with_memory(&self.module, mem);
                it.fuel = FUEL;
                let value = f64::from_bits(it.call(self.derivative, &words)?);
                let all = it.mem.read(buffer, self.param_tys.len());
                Ok((value, self.active.iter().map(|&i| all[i]).collect()))
            }
            Mode::Forward => {
                let value = self.primal_value(at)?;
                let mut gradient = Vec::with_capacity(self.active.len());
                for &seeded in &self.active {
                    let mut words = Vec::new();
                    for (i, (&ty, &x)) in self.param_tys.iter().zip(at).enumerate() {
                        words.push(Self::word(ty, x));
                        if self.active.contains(&i) {
                            words.push(if i == seeded { 1.0f64 } else { 0.0 }.to_bits());
                        }
                    }
                    let mut it = self.interpreter();
                    gradient.push(f64::from_bits(it.call(self.derivative, &words)?));
                }
                Ok((value, gradient))
            }
        }
    }
}
