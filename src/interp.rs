//! Executing the IR.
//!
//! This is the reference semantics. Everything else in the crate -- the
//! optimiser, both AD transforms, the native code path -- is only correct
//! insofar as it agrees with what this file does, and the tests say so by
//! running both and comparing.
//!
//! It is a straightforward block-walking interpreter, deliberately: it is the
//! thing that has to be obviously right, not the thing that has to be fast.
//! Speed is [`crate::native`]'s job.

use crate::ir::*;

/// Linear memory: one flat array of 8-byte slots, shared by every frame.
///
/// A pointer is an index into this. `Alloca` bumps the high-water mark and
/// never frees, which is what a program being differentiated wants anyway --
/// the reverse pass may need to read a buffer the forward pass finished with.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub slots: Vec<u64>,
}

impl Memory {
    pub fn with_capacity(n: usize) -> Self {
        Memory { slots: vec![0; n] }
    }
    pub fn alloc(&mut self, n: usize) -> u64 {
        let at = self.slots.len();
        self.slots.resize(at + n.max(1), 0);
        at as u64
    }
    pub fn get_real(&self, p: u64) -> f64 {
        f64::from_bits(self.slots[p as usize])
    }
    pub fn set_real(&mut self, p: u64, x: f64) {
        self.slots[p as usize] = x.to_bits();
    }
    pub fn read(&self, p: u64, n: usize) -> Vec<f64> {
        (0..n).map(|i| self.get_real(p + i as u64)).collect()
    }
    pub fn write(&mut self, p: u64, xs: &[f64]) {
        for (i, x) in xs.iter().enumerate() {
            self.set_real(p + i as u64, *x);
        }
    }
    /// Put a vector somewhere and hand back the pointer.
    pub fn place(&mut self, xs: &[f64]) -> u64 {
        let p = self.alloc(xs.len());
        self.write(p, xs);
        p
    }
}

/// Why a run stopped, when it did not finish.
#[derive(Debug, Clone, PartialEq)]
pub enum Fault {
    OutOfFuel(u64),
    BadPointer { at: Value, slot: u64 },
    TooDeep,
    NoReturn,
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::OutOfFuel(n) => write!(f, "ran {n} steps without terminating"),
            Fault::BadPointer { at, slot } => {
                write!(f, "%{at} touched slot {slot}, which is not allocated")
            }
            Fault::TooDeep => write!(f, "call depth exceeded"),
            Fault::NoReturn => write!(f, "a function returning a value fell off a void return"),
        }
    }
}
impl std::error::Error for Fault {}

/// One run of the interpreter.
pub struct Interp<'m> {
    pub module: &'m Module,
    pub mem: Memory,
    /// Instructions retired. The honest cost measure for the optimiser
    /// benchmarks: wall clock on an interpreter measures the interpreter.
    pub steps: u64,
    pub fuel: u64,
    depth: u32,
}

const MAX_DEPTH: u32 = 256;

impl<'m> Interp<'m> {
    pub fn new(module: &'m Module) -> Self {
        Interp {
            module,
            mem: Memory::default(),
            steps: 0,
            fuel: 1 << 30,
            depth: 0,
        }
    }

    pub fn with_memory(module: &'m Module, mem: Memory) -> Self {
        let mut i = Interp::new(module);
        i.mem = mem;
        i
    }

    /// Run `f` on `args` (raw register words) and return its result word.
    pub fn call(&mut self, id: FuncId, args: &[u64]) -> Result<u64, Fault> {
        if self.depth >= MAX_DEPTH {
            return Err(Fault::TooDeep);
        }
        self.depth += 1;
        let out = self.run(id, args);
        self.depth -= 1;
        out
    }

    /// The convenience the tests and the C ABI actually want: reals in, real
    /// out.
    pub fn call_real(&mut self, id: FuncId, args: &[f64]) -> Result<f64, Fault> {
        let words: Vec<u64> = args
            .iter()
            .zip(self.module.get(id).params.iter())
            .map(|(x, t)| match t {
                Ty::Real => x.to_bits(),
                _ => *x as i64 as u64,
            })
            .collect();
        Ok(f64::from_bits(self.call(id, &words)?))
    }

    fn run(&mut self, id: FuncId, args: &[u64]) -> Result<u64, Fault> {
        let f = self.module.get(id);
        let mut reg = vec![0u64; f.insts.len()];
        let mut block = f.entry;
        // Phis read from the edge, so the block we arrived from is state.
        let mut came_from: Option<BlockId> = None;

        loop {
            for &v in &f.blocks[block as usize].insts {
                self.steps += 1;
                if self.steps > self.fuel {
                    return Err(Fault::OutOfFuel(self.steps));
                }
                let r = |x: Value| reg[x as usize];
                let rf = |x: Value| f64::from_bits(reg[x as usize]);
                let ri = |x: Value| reg[x as usize] as i64;

                let out: u64 = match f.op(v) {
                    Op::ConstReal(c) => c.to_bits(),
                    Op::ConstInt(c) => *c as u64,
                    Op::Param(i) => *args.get(*i as usize).unwrap_or(&0),
                    Op::Un(o, a) => o.eval(rf(*a)).to_bits(),
                    Op::Bin(o, a, b) => o.eval(rf(*a), rf(*b)).to_bits(),
                    Op::Int(o, a, b) => o.eval(ri(*a), ri(*b)) as u64,
                    Op::FCmp(p, a, b) => u64::from(p.apply(rf(*a), rf(*b))),
                    Op::ICmp(p, a, b) => u64::from(p.apply_int(ri(*a), ri(*b))),
                    Op::Select(c, a, b) => {
                        if r(*c) != 0 {
                            r(*a)
                        } else {
                            r(*b)
                        }
                    }
                    Op::Alloca(n) => self.mem.alloc(ri(*n).max(0) as usize),
                    Op::Gep(p, o) => (r(*p) as i64).wrapping_add(ri(*o)) as u64,
                    Op::Load(p) => {
                        let slot = r(*p);
                        *self
                            .mem
                            .slots
                            .get(slot as usize)
                            .ok_or(Fault::BadPointer { at: v, slot })?
                    }
                    Op::Store(p, x) => {
                        let slot = r(*p);
                        let cell = self
                            .mem
                            .slots
                            .get_mut(slot as usize)
                            .ok_or(Fault::BadPointer { at: v, slot })?;
                        *cell = r(*x);
                        0
                    }
                    Op::Phi(inc) => {
                        let from = came_from.expect("a phi in the entry block has no edge");
                        let mut got = 0;
                        for (b, x) in inc {
                            if *b == from {
                                got = reg[*x as usize];
                                break;
                            }
                        }
                        got
                    }
                    Op::Call(g, argv) => {
                        let vals: Vec<u64> = argv.iter().map(|a| reg[*a as usize]).collect();
                        self.call(*g, &vals)?
                    }
                    Op::IntToReal(a) => (ri(*a) as f64).to_bits(),
                    Op::RealToInt(a) => rf(*a) as i64 as u64,
                };
                reg[v as usize] = out;
            }

            match &f.blocks[block as usize].term {
                Term::Br(t) => {
                    came_from = Some(block);
                    block = *t;
                }
                Term::CondBr(c, t, e) => {
                    let next = if reg[*c as usize] != 0 { *t } else { *e };
                    came_from = Some(block);
                    block = next;
                }
                Term::Ret(Some(v)) => return Ok(reg[*v as usize]),
                Term::Ret(None) => {
                    return if f.ret == Ty::Void {
                        Ok(0)
                    } else {
                        Err(Fault::NoReturn)
                    }
                }
                Term::Unset => return Err(Fault::NoReturn),
            }
        }
    }
}

/// Run a function and report both its answer and what it cost.
pub fn run_counting(module: &Module, id: FuncId, args: &[f64]) -> (Result<f64, Fault>, u64) {
    let mut it = Interp::new(module);
    let out = it.call_real(id, args);
    (out, it.steps)
}
