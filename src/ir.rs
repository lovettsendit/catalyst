//! The intermediate representation everything else operates on.
//!
//! # Why it looks like this
//!
//! Enzyme differentiates LLVM IR *after* the optimiser has finished with it,
//! and the difficulty of the problem comes almost entirely from what that IR
//! looks like by then. So this IR reproduces the properties that make it hard,
//! rather than a comfortable expression tree that would make the AD easy and
//! the result meaningless:
//!
//! * **SSA with a real CFG.** Blocks, conditional branches, phi nodes, loops.
//!   Reverse mode over straight-line code is an afternoon; reverse mode over a
//!   CFG needs a control-flow tape, which is the interesting part.
//! * **Untyped memory.** [`Op::Load`] yields eight raw bytes. Nothing in the
//!   instruction says whether those bytes are a float carrying a derivative or
//!   a loop counter that does not. That is exactly LLVM's situation after
//!   pointer types were erased, and it is why [`crate::typeanalysis`] exists.
//! * **Flat linear memory.** One address space of 8-byte slots, `Alloca` bumps
//!   it, `Gep` does arithmetic on it. Aliasing is possible, so the analyses
//!   have to cope with it rather than assume it away.
//!
//! Registers are `u64` everywhere. A real is the bit pattern of an `f64`; an
//! integer is the bit pattern of an `i64`; a pointer is a slot index. The
//! *operation* decides how to read its operands, which is the same contract
//! LLVM has and the reason type analysis is load-bearing rather than
//! decorative.

use std::collections::HashMap;
use std::fmt::Write as _;

/// An SSA value: an index into [`Func::insts`].
pub type Value = u32;
/// An index into [`Func::blocks`].
pub type BlockId = u32;
/// An index into [`Module::funcs`].
pub type FuncId = u32;

/// What a value *is*, as far as the instruction set is concerned.
///
/// This is the shallow type: it tells you how the eight bytes in the register
/// are to be read. It says nothing about what a pointer points *to*, which is
/// the question [`crate::typeanalysis`] answers and this enum cannot.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    /// An `f64`, stored as its bit pattern.
    Real,
    /// An `i64`, stored as its bit pattern. Indices, counters, predicates.
    Int,
    /// A slot index into linear memory.
    Ptr,
    /// The result of a store, or of a call returning nothing.
    Void,
}

/// Comparison predicates, shared by the real and integer comparisons.
///
/// The four unsigned orderings exist for [`Op::ICmp`] alone: an integer
/// register holds an `i64` bit pattern, and a compiler that counts with
/// unsigned arithmetic (`icmp ult` is what LLVM writes for a trip count it
/// knows is non-negative) compares those bits as a `u64`. On a real they mean
/// nothing, and [`Pred::apply`] treats them as their signed twins so that a
/// misuse is a wrong ordering rather than a panic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pred {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    ULt,
    ULe,
    UGt,
    UGe,
}

impl Pred {
    pub fn symbol(self) -> &'static str {
        match self {
            Pred::Lt => "lt",
            Pred::Le => "le",
            Pred::Gt => "gt",
            Pred::Ge => "ge",
            Pred::Eq => "eq",
            Pred::Ne => "ne",
            Pred::ULt => "ult",
            Pred::ULe => "ule",
            Pred::UGt => "ugt",
            Pred::UGe => "uge",
        }
    }
    pub fn apply<T: PartialOrd>(self, a: T, b: T) -> bool {
        match self {
            Pred::Lt | Pred::ULt => a < b,
            Pred::Le | Pred::ULe => a <= b,
            Pred::Gt | Pred::UGt => a > b,
            Pred::Ge | Pred::UGe => a >= b,
            Pred::Eq => a == b,
            Pred::Ne => a != b,
        }
    }
    /// The integer comparison: signed for the six orderings every language
    /// has, and over the same bits read as `u64` for the four unsigned ones.
    pub fn apply_int(self, a: i64, b: i64) -> bool {
        match self {
            Pred::ULt | Pred::ULe | Pred::UGt | Pred::UGe => self.apply(a as u64, b as u64),
            _ => self.apply(a, b),
        }
    }
    /// Whether this is one of the four unsigned orderings.
    pub fn unsigned(self) -> bool {
        matches!(self, Pred::ULt | Pred::ULe | Pred::UGt | Pred::UGe)
    }
}

/// The transcendental and algebraic unary operations.
///
/// They are one variant with a discriminant rather than fifteen variants
/// because every pass wants to say "some unary real op" and then ask for its
/// derivative; splitting them costs a fifteen-arm match in every pass and buys
/// nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Sin,
    Cos,
    Tan,
    Exp,
    Log,
    Sqrt,
    Tanh,
    Sinh,
    Cosh,
    Abs,
    Recip,
    Erf,
}

impl UnOp {
    pub fn name(self) -> &'static str {
        match self {
            UnOp::Neg => "neg",
            UnOp::Sin => "sin",
            UnOp::Cos => "cos",
            UnOp::Tan => "tan",
            UnOp::Exp => "exp",
            UnOp::Log => "log",
            UnOp::Sqrt => "sqrt",
            UnOp::Tanh => "tanh",
            UnOp::Sinh => "sinh",
            UnOp::Cosh => "cosh",
            UnOp::Abs => "abs",
            UnOp::Recip => "recip",
            UnOp::Erf => "erf",
        }
    }

    pub fn eval(self, x: f64) -> f64 {
        match self {
            UnOp::Neg => -x,
            UnOp::Sin => x.sin(),
            UnOp::Cos => x.cos(),
            UnOp::Tan => x.tan(),
            UnOp::Exp => x.exp(),
            UnOp::Log => x.ln(),
            UnOp::Sqrt => x.sqrt(),
            UnOp::Tanh => x.tanh(),
            UnOp::Sinh => x.sinh(),
            UnOp::Cosh => x.cosh(),
            UnOp::Abs => x.abs(),
            UnOp::Recip => 1.0 / x,
            UnOp::Erf => erf(x),
        }
    }
}

/// The binary real operations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Max,
    Min,
    Atan2,
}

impl BinOp {
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Pow => "pow",
            BinOp::Max => "max",
            BinOp::Min => "min",
            BinOp::Atan2 => "atan2",
        }
    }

    pub fn eval(self, a: f64, b: f64) -> f64 {
        match self {
            BinOp::Add => a + b,
            BinOp::Sub => a - b,
            BinOp::Mul => a * b,
            BinOp::Div => a / b,
            BinOp::Pow => a.powf(b),
            BinOp::Max => {
                if a >= b {
                    a
                } else {
                    b
                }
            }
            BinOp::Min => {
                if a <= b {
                    a
                } else {
                    b
                }
            }
            BinOp::Atan2 => a.atan2(b),
        }
    }

    /// Whether reordering the operands changes the answer. Used by the value
    /// numbering in [`crate::opt`] to canonicalise before hashing.
    pub fn commutative(self) -> bool {
        matches!(self, BinOp::Add | BinOp::Mul | BinOp::Max | BinOp::Min)
    }
}

/// Integer operations. Kept separate from the real ones on purpose: an
/// instruction that is integral by construction can never be active, and
/// activity analysis gets that for free rather than deriving it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntOp {
    Add,
    Sub,
    Mul,
    /// Signed division, truncating towards zero; zero when the divisor is.
    Div,
    /// Signed remainder, with the sign of the dividend; zero when the divisor is.
    Rem,
    /// Division over the same bits read as `u64`.
    UDiv,
    /// Remainder over the same bits read as `u64`.
    URem,
    And,
    Or,
    Xor,
    Shl,
    /// Arithmetic right shift: the sign bit is copied in.
    Shr,
    /// Logical right shift: zeros are shifted in.
    LShr,
}

impl IntOp {
    pub fn name(self) -> &'static str {
        match self {
            IntOp::Add => "iadd",
            IntOp::Sub => "isub",
            IntOp::Mul => "imul",
            IntOp::Div => "idiv",
            IntOp::Rem => "irem",
            IntOp::UDiv => "udiv",
            IntOp::URem => "urem",
            IntOp::And => "and",
            IntOp::Or => "or",
            IntOp::Xor => "xor",
            IntOp::Shl => "shl",
            IntOp::Shr => "shr",
            IntOp::LShr => "lshr",
        }
    }
    pub fn eval(self, a: i64, b: i64) -> i64 {
        match self {
            IntOp::Add => a.wrapping_add(b),
            IntOp::Sub => a.wrapping_sub(b),
            IntOp::Mul => a.wrapping_mul(b),
            IntOp::Div => {
                if b == 0 {
                    0
                } else {
                    a.wrapping_div(b)
                }
            }
            IntOp::Rem => {
                if b == 0 {
                    0
                } else {
                    a.wrapping_rem(b)
                }
            }
            IntOp::UDiv => {
                if b == 0 {
                    0
                } else {
                    ((a as u64) / (b as u64)) as i64
                }
            }
            IntOp::URem => {
                if b == 0 {
                    0
                } else {
                    ((a as u64) % (b as u64)) as i64
                }
            }
            IntOp::And => a & b,
            IntOp::Or => a | b,
            IntOp::Xor => a ^ b,
            IntOp::Shl => a.wrapping_shl(b as u32),
            IntOp::Shr => a.wrapping_shr(b as u32),
            IntOp::LShr => (a as u64).wrapping_shr(b as u32) as i64,
        }
    }
}

/// One instruction.
#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    /// A literal real.
    ConstReal(f64),
    /// A literal integer.
    ConstInt(i64),
    /// Formal parameter `n` of the enclosing function.
    Param(u32),
    /// A real unary operation.
    Un(UnOp, Value),
    /// A real binary operation.
    Bin(BinOp, Value, Value),
    /// An integer binary operation.
    Int(IntOp, Value, Value),
    /// Compare two reals, yielding `Int` 0 or 1.
    FCmp(Pred, Value, Value),
    /// Compare two integers, yielding `Int` 0 or 1.
    ICmp(Pred, Value, Value),
    /// `cond ? a : b`, on any type. Differentiable when its arms are.
    Select(Value, Value, Value),
    /// Reserve `n` slots of linear memory and yield a pointer to the first.
    Alloca(Value),
    /// Pointer arithmetic: `ptr + offset` slots.
    Gep(Value, Value),
    /// Read eight raw bytes. What they *mean* is not stated here.
    Load(Value),
    /// Write eight raw bytes. Yields [`Ty::Void`].
    Store(Value, Value),
    /// An SSA phi: one incoming value per predecessor block.
    Phi(Vec<(BlockId, Value)>),
    /// Call another function in the module.
    Call(FuncId, Vec<Value>),
    /// Reinterpret an integer as a real, and back. Real conversions, not
    /// bitcasts: these are the honest `sitofp`/`fptosi`.
    IntToReal(Value),
    RealToInt(Value),
}

/// How a block ends. Every block has exactly one.
#[derive(Clone, PartialEq, Debug)]
pub enum Term {
    Br(BlockId),
    CondBr(Value, BlockId, BlockId),
    Ret(Option<Value>),
    /// Only valid in a partly-built function; the verifier rejects it.
    Unset,
}

#[derive(Clone, Debug)]
pub struct Inst {
    pub op: Op,
    pub ty: Ty,
    /// Which block this instruction lives in. Kept on the instruction so that
    /// any pass holding a bare [`Value`] can ask where it is without a map.
    pub block: BlockId,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub label: String,
    pub insts: Vec<Value>,
    pub term: Term,
}

/// A function: an arena of instructions plus the blocks that order them.
#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub insts: Vec<Inst>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
}

impl Func {
    pub fn new(name: &str, params: Vec<Ty>, ret: Ty) -> Self {
        let mut f = Func {
            name: name.to_owned(),
            params,
            ret,
            insts: Vec::new(),
            blocks: Vec::new(),
            entry: 0,
        };
        f.new_block("entry");
        f
    }

    pub fn new_block(&mut self, label: &str) -> BlockId {
        let id = self.blocks.len() as BlockId;
        self.blocks.push(Block {
            label: format!("{label}{id}"),
            insts: Vec::new(),
            term: Term::Unset,
        });
        id
    }

    /// Append an instruction to a block. The only way a value is created.
    pub fn push(&mut self, block: BlockId, op: Op, ty: Ty) -> Value {
        let v = self.insts.len() as Value;
        self.insts.push(Inst { op, ty, block });
        self.blocks[block as usize].insts.push(v);
        v
    }

    /// Insert at the front of a block, which is where phis have to go.
    pub fn push_front(&mut self, block: BlockId, op: Op, ty: Ty) -> Value {
        let v = self.insts.len() as Value;
        self.insts.push(Inst { op, ty, block });
        let at = self.blocks[block as usize]
            .insts
            .iter()
            .position(|&i| !matches!(self.insts[i as usize].op, Op::Phi(_)))
            .unwrap_or(self.blocks[block as usize].insts.len());
        self.blocks[block as usize].insts.insert(at, v);
        v
    }

    pub fn op(&self, v: Value) -> &Op {
        &self.insts[v as usize].op
    }
    pub fn ty(&self, v: Value) -> Ty {
        self.insts[v as usize].ty
    }
    pub fn block_of(&self, v: Value) -> BlockId {
        self.insts[v as usize].block
    }

    /// The successors of a block, in branch order.
    pub fn succs(&self, b: BlockId) -> Vec<BlockId> {
        match &self.blocks[b as usize].term {
            Term::Br(t) => vec![*t],
            Term::CondBr(_, t, e) => vec![*t, *e],
            Term::Ret(_) | Term::Unset => vec![],
        }
    }

    /// Predecessors, computed on demand. Cheap at these sizes and always
    /// correct, which a cached copy would not be after every transform.
    pub fn preds(&self) -> Vec<Vec<BlockId>> {
        let mut p = vec![Vec::new(); self.blocks.len()];
        for b in 0..self.blocks.len() as BlockId {
            for s in self.succs(b) {
                p[s as usize].push(b);
            }
        }
        p
    }

    /// Blocks in reverse post-order: every block before its non-back-edge
    /// successors. The order every forward dataflow analysis wants.
    pub fn rpo(&self) -> Vec<BlockId> {
        let mut seen = vec![false; self.blocks.len()];
        let mut post = Vec::new();
        // Iterative, because a deep CFG would otherwise recurse to the stack
        // limit on exactly the large functions this is meant to handle.
        let mut stack = vec![(self.entry, 0usize)];
        seen[self.entry as usize] = true;
        while let Some((b, i)) = stack.pop() {
            let succs = self.succs(b);
            if i < succs.len() {
                stack.push((b, i + 1));
                let s = succs[i];
                if !seen[s as usize] {
                    seen[s as usize] = true;
                    stack.push((s, 0));
                }
            } else {
                post.push(b);
            }
        }
        post.reverse();
        post
    }

    /// Every value an instruction reads. The single place operand order is
    /// defined, so that no pass has to re-derive it and get it wrong.
    pub fn operands(&self, v: Value) -> Vec<Value> {
        match &self.insts[v as usize].op {
            Op::ConstReal(_) | Op::ConstInt(_) | Op::Param(_) => vec![],
            Op::Un(_, a) | Op::IntToReal(a) | Op::RealToInt(a) | Op::Alloca(a) | Op::Load(a) => {
                vec![*a]
            }
            Op::Bin(_, a, b)
            | Op::Int(_, a, b)
            | Op::FCmp(_, a, b)
            | Op::ICmp(_, a, b)
            | Op::Gep(a, b)
            | Op::Store(a, b) => vec![*a, *b],
            Op::Select(c, a, b) => vec![*c, *a, *b],
            Op::Phi(incoming) => incoming.iter().map(|(_, v)| *v).collect(),
            Op::Call(_, args) => args.clone(),
        }
    }

    /// Rewrite every operand through `map`. Used by every transform that
    /// clones code -- inlining, unrolling, and both AD passes.
    pub fn remap_operands(&mut self, v: Value, map: &HashMap<Value, Value>) {
        let get = |x: &Value| *map.get(x).unwrap_or(x);
        match &mut self.insts[v as usize].op {
            Op::ConstReal(_) | Op::ConstInt(_) | Op::Param(_) => {}
            Op::Un(_, a) | Op::IntToReal(a) | Op::RealToInt(a) | Op::Alloca(a) | Op::Load(a) => {
                *a = get(a)
            }
            Op::Bin(_, a, b)
            | Op::Int(_, a, b)
            | Op::FCmp(_, a, b)
            | Op::ICmp(_, a, b)
            | Op::Gep(a, b)
            | Op::Store(a, b) => {
                *a = get(a);
                *b = get(b);
            }
            Op::Select(c, a, b) => {
                *c = get(c);
                *a = get(a);
                *b = get(b);
            }
            Op::Phi(incoming) => {
                for (_, x) in incoming.iter_mut() {
                    *x = get(x);
                }
            }
            Op::Call(_, args) => {
                for x in args.iter_mut() {
                    *x = get(x);
                }
            }
        }
    }

    /// Whether removing this instruction could change what the program does.
    /// Stores and calls, and nothing else: the IR has no other effects.
    pub fn has_effect(&self, v: Value) -> bool {
        matches!(self.insts[v as usize].op, Op::Store(_, _) | Op::Call(_, _))
    }

    /// Instructions actually reachable from the entry block, in program order.
    pub fn live_insts(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for b in self.rpo() {
            out.extend_from_slice(&self.blocks[b as usize].insts);
        }
        out
    }

    /// The size of the function, as the benchmarks count it.
    pub fn inst_count(&self) -> usize {
        self.live_insts().len()
    }
}

/// A whole program: functions that may call each other.
#[derive(Clone, Debug, Default)]
pub struct Module {
    pub funcs: Vec<Func>,
}

impl Module {
    pub fn new() -> Self {
        Module::default()
    }
    pub fn add(&mut self, f: Func) -> FuncId {
        self.funcs.push(f);
        (self.funcs.len() - 1) as FuncId
    }
    pub fn get(&self, id: FuncId) -> &Func {
        &self.funcs[id as usize]
    }
    pub fn get_mut(&mut self, id: FuncId) -> &mut Func {
        &mut self.funcs[id as usize]
    }
    pub fn find(&self, name: &str) -> Option<FuncId> {
        self.funcs
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as FuncId)
    }
}

/// A structural check, run in the tests after every transform.
///
/// The AD passes build IR out of other IR, and a transform that produces a
/// use-before-def or a phi whose incoming blocks no longer match its
/// predecessors will otherwise fail later, somewhere else, as a wrong number.
pub fn verify(module: &Module, id: FuncId) -> Result<(), String> {
    let f = module.get(id);
    let preds = f.preds();
    let mut defined = vec![false; f.insts.len()];
    for b in f.rpo() {
        for &v in &f.blocks[b as usize].insts {
            if let Op::Phi(incoming) = f.op(v) {
                let mut have: Vec<BlockId> = incoming.iter().map(|(b, _)| *b).collect();
                let mut want = preds[b as usize].clone();
                have.sort_unstable();
                want.sort_unstable();
                if have != want {
                    return Err(format!(
                        "{}: phi %{v} in {} lists blocks {have:?} but its predecessors are {want:?}",
                        f.name, f.blocks[b as usize].label
                    ));
                }
                // A phi's operands are defined on the edge, not here, so they
                // are exempt from the dominance walk below.
                for (_, x) in incoming {
                    if *x as usize >= f.insts.len() {
                        return Err(format!("{}: phi %{v} reads %{x}, out of range", f.name));
                    }
                }
            } else {
                for x in f.operands(v) {
                    if x as usize >= f.insts.len() {
                        return Err(format!("{}: %{v} reads %{x}, out of range", f.name));
                    }
                    if !defined[x as usize] {
                        return Err(format!(
                            "{}: %{v} in {} reads %{x} before it is defined",
                            f.name, f.blocks[b as usize].label
                        ));
                    }
                }
            }
            defined[v as usize] = true;
        }
        if matches!(f.blocks[b as usize].term, Term::Unset) {
            return Err(format!(
                "{}: {} has no terminator",
                f.name, f.blocks[b as usize].label
            ));
        }
        for s in f.succs(b) {
            if s as usize >= f.blocks.len() {
                return Err(format!(
                    "{}: {} branches to a block that does not exist",
                    f.name, f.blocks[b as usize].label
                ));
            }
        }
    }
    Ok(())
}

/// Render a function as text. Used by the logs, the documentation and the
/// before/after diffs in the benchmarks, so it has to be readable rather than
/// merely complete.
pub fn print_func(module: &Module, id: FuncId) -> String {
    let f = module.get(id);
    let mut s = String::new();
    let tys: Vec<String> = f
        .params
        .iter()
        .map(|t| format!("{t:?}").to_lowercase())
        .collect();
    let _ = writeln!(s, "func @{}({}) -> {:?} {{", f.name, tys.join(", "), f.ret);
    for b in f.rpo() {
        let _ = writeln!(s, "  {}:", f.blocks[b as usize].label);
        for &v in &f.blocks[b as usize].insts {
            let text = match f.op(v) {
                Op::ConstReal(c) => format!("{c:?}"),
                Op::ConstInt(c) => format!("{c}"),
                Op::Param(i) => format!("param {i}"),
                Op::Un(o, a) => format!("{} %{a}", o.name()),
                Op::Bin(o, a, b) => format!("{} %{a}, %{b}", o.name()),
                Op::Int(o, a, b) => format!("{} %{a}, %{b}", o.name()),
                Op::FCmp(p, a, b) => format!("fcmp.{} %{a}, %{b}", p.symbol()),
                Op::ICmp(p, a, b) => format!("icmp.{} %{a}, %{b}", p.symbol()),
                Op::Select(c, a, b) => format!("select %{c}, %{a}, %{b}"),
                Op::Alloca(n) => format!("alloca %{n}"),
                Op::Gep(p, o) => format!("gep %{p}, %{o}"),
                Op::Load(p) => format!("load %{p}"),
                Op::Store(p, x) => format!("store %{p}, %{x}"),
                Op::Phi(inc) => {
                    let parts: Vec<String> = inc
                        .iter()
                        .map(|(b, v)| {
                            format!("[{}, %{v}]", module.get(id).blocks[*b as usize].label)
                        })
                        .collect();
                    format!("phi {}", parts.join(", "))
                }
                Op::Call(g, args) => {
                    let parts: Vec<String> = args.iter().map(|a| format!("%{a}")).collect();
                    format!("call @{}({})", module.get(*g).name, parts.join(", "))
                }
                Op::IntToReal(a) => format!("sitofp %{a}"),
                Op::RealToInt(a) => format!("fptosi %{a}"),
            };
            if f.ty(v) == Ty::Void {
                let _ = writeln!(s, "    {text}");
            } else {
                let _ = writeln!(s, "    %{v} = {text}");
            }
        }
        let term = match &f.blocks[b as usize].term {
            Term::Br(t) => format!("br {}", f.blocks[*t as usize].label),
            Term::CondBr(c, t, e) => format!(
                "condbr %{c}, {}, {}",
                f.blocks[*t as usize].label, f.blocks[*e as usize].label
            ),
            Term::Ret(Some(v)) => format!("ret %{v}"),
            Term::Ret(None) => "ret".to_owned(),
            Term::Unset => "<unset>".to_owned(),
        };
        let _ = writeln!(s, "    {term}");
    }
    let _ = writeln!(s, "}}");
    s
}

/// `ln Gamma(1/2)`, which is `ln sqrt(pi)`. Written as a constant because it is
/// the only value of `lgamma` this file needs and carrying a whole `lgamma`
/// for it would be a poor trade in a crate with no dependencies.
const LN_GAMMA_HALF: f64 = 0.572_364_942_924_700_1;

/// The regularised incomplete gamma `P(1/2, x)`, by its series. Convergent
/// quickly for small `x`, which is the half of the domain it is used on.
fn gamma_p_half_series(x: f64) -> f64 {
    let a = 0.5;
    let mut ap = a;
    let mut sum = 1.0 / a;
    let mut del = sum;
    // 300 is a bound, not an expectation: the series settles in well under 30
    // terms across the range it is used on, and the bound only exists so a
    // pathological input cannot spin here.
    for _ in 0..300 {
        ap += 1.0;
        del *= x / ap;
        sum += del;
        if del.abs() < sum.abs() * 1e-17 {
            break;
        }
    }
    sum * (-x + a * x.ln() - LN_GAMMA_HALF).exp()
}

/// The complement `Q(1/2, x)`, by its continued fraction in the modified
/// Lentz form. This is the accurate half for large `x`, where the series
/// above loses digits to cancellation.
fn gamma_q_half_fraction(x: f64) -> f64 {
    let a = 0.5;
    // Small enough to stand in for zero in a denominator without perturbing
    // the result, and large enough not to underflow when inverted.
    const TINY: f64 = 1e-300;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / TINY;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..300 {
        let i = f64::from(i);
        let an = -i * (i - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < TINY {
            d = TINY;
        }
        c = b + an / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-17 {
            break;
        }
    }
    (-x + a * x.ln() - LN_GAMMA_HALF).exp() * h
}

/// The error function, to full double precision.
///
/// # Why this is not the usual four-line approximation
///
/// It used to be Abramowitz & Stegun 7.1.26, whose stated maximum error is
/// **1.5e-7**. That is single precision in an `f64` pipeline, and it is not
/// merely imprecise: it makes the primal disagree with its own derivative.
/// Both AD transforms differentiate `erf` with the exact analytic rule,
/// `2/sqrt(pi) * exp(-x^2)`, so with an approximate primal the transform
/// returns the derivative of a function the interpreter does not compute.
/// `tests/reverse.rs::every_rule_matches_central_differences_on_its_own`
/// measured the gap at 1.5e-6 relative -- a thousand times the tolerance, and
/// nothing to do with the AD rule, which was right all along.
///
/// `erf(x) = sign(x) * P(1/2, x^2)`, with the series below the crossover and
/// the continued fraction above it, each used on the side where it keeps its
/// digits. Both agree with the other to the last bit or two at the crossover,
/// which is the property that makes the choice of crossover unimportant.
fn erf(x: f64) -> f64 {
    let z = x * x;
    let p = if z < 1.5 {
        gamma_p_half_series(z)
    } else {
        1.0 - gamma_q_half_fraction(z)
    };
    if x < 0.0 {
        -p
    } else {
        p
    }
}
