//! Lowering read LLVM IR onto the engine's own IR.
//!
//! The engine's IR is smaller than LLVM's on purpose, and the distance between
//! the two is what this file crosses:
//!
//! * LLVM has integer types of several widths; the engine has one `Int`
//!   register of sixty-four bits. `zext`, `sext` and `trunc` between the
//!   integer types are therefore identities here, and `i1` is the pair 0, 1.
//! * LLVM has named values; the engine has an arena. Every `%name` becomes an
//!   index, constants become instructions in the entry block -- where they
//!   dominate every use, including the phis that read them on an edge -- and
//!   a phi's incoming list is stitched after every block has been emitted,
//!   because a loop's back edge names a value the text has not defined yet.
//! * LLVM has calls; the engine differentiates one function. A call to a
//!   function defined in the module is inlined here, so that the derivative
//!   passes through it, unless a custom rule names the callee, in which case
//!   the call stays a call and the transforms are handed the rule. A call to
//!   an intrinsic or a libm name is the engine's own operation. Anything else
//!   is an opaque call and is refused by name before anything runs.
//!
//! Blocks are emitted in reverse post-order over the LLVM control flow graph,
//! not in textual order: `rustc` writes a loop's body last and the block that
//! reads its result first, and only an order in which every dominator comes
//! before what it dominates lets a plain `%name` lookup find its definition.

use super::llvm::{self, Cast, FloatOp, Inst, IntegerOp, Operand, Terminator, Type};
use crate::ir::{BinOp, BlockId, Func, FuncId, IntOp, Module, Op, Pred, Term, Ty, UnOp, Value};
use std::collections::HashMap;

/// Why the function could not be lowered. Each is one refusal code at the
/// command line; the line is the LLVM line the refusal points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LowerError {
    /// Outside the phase-1 subset: the detail names the construct.
    Unsupported { what: String, line: usize },
    /// A call to a function the engine cannot see through.
    OpaqueCall { callee: String, line: usize },
    /// The text was well-formed line by line but not as a program: a name
    /// used before any definition, a branch to a block that does not exist.
    Malformed { what: String, line: usize },
}

/// A custom rule, resolved to the two functions it names.
#[derive(Clone, Debug)]
pub struct RuleBinding {
    pub function: String,
    pub derivative: String,
}

/// The function, lowered, with the module it lives in.
pub struct Lowered {
    pub module: Module,
    /// The lowered function.
    pub id: FuncId,
    /// The rules that applied: the callee's name, its id, and its
    /// derivative's id, in the order the rules were given.
    pub rules_applied: Vec<(String, FuncId, FuncId)>,
}

/// The engine type of a scalar LLVM type.
fn engine_ty(ty: &Type) -> Option<Ty> {
    match ty {
        Type::Double => Some(Ty::Real),
        Type::Int(_) => Some(Ty::Int),
        Type::Void | Type::Other(_) => None,
    }
}

/// Lower `function` from `module`, applying `rules`.
///
/// The rule functions themselves are lowered first, as functions of their
/// own, so that the primal can call the real `f` and the derivative the
/// supplied `g`.
pub fn lower(
    module: &llvm::Module,
    function: &str,
    rules: &[RuleBinding],
) -> Result<Lowered, LowerError> {
    let mut lowering = Lowering {
        source: module,
        out: Module::new(),
        rule_ids: HashMap::new(),
        rules_applied: Vec::new(),
    };
    for rule in rules {
        let f = lowering.standalone(&rule.function)?;
        let g = lowering.standalone(&rule.derivative)?;
        lowering.rule_ids.insert(rule.function.clone(), (f, g));
    }
    let id = lowering.standalone(function)?;
    // A rule counts as applied when the function under differentiation, or
    // anything inlined into it, actually called the rule's function.
    let mut applied = Vec::new();
    for rule in rules {
        if lowering.rules_applied.contains(&rule.function) {
            let (f, g) = lowering.rule_ids[&rule.function];
            applied.push((rule.function.clone(), f, g));
        }
    }
    Ok(Lowered {
        module: lowering.out,
        id,
        rules_applied: applied,
    })
}

struct Lowering<'a> {
    source: &'a llvm::Module,
    out: Module,
    /// Rule callee name -> (its lowered id, its derivative's lowered id).
    rule_ids: HashMap<String, (FuncId, FuncId)>,
    rules_applied: Vec<String>,
}

/// One function being built: the arena, the constant cache, and the name
/// table of the function currently being emitted into it.
struct Builder<'a> {
    f: Func,
    consts: HashMap<(bool, u64), Value>,
    source: &'a llvm::Module,
    rule_ids: &'a HashMap<String, (FuncId, FuncId)>,
    rules_applied: &'a mut Vec<String>,
}

/// The state of one function body being emitted -- the top-level one, or a
/// callee being inlined into it.
struct Frame {
    env: HashMap<String, Value>,
    /// Where a branch to the labelled block goes.
    starts: HashMap<String, BlockId>,
    /// Which block the labelled block's instructions *end* in, which differs
    /// from `starts` once an inlined call has split it.
    ends: HashMap<String, BlockId>,
    /// Phis to stitch once every value exists.
    phis: Vec<PendingPhi>,
}

/// A phi whose incoming list is stitched once every value exists: the phi,
/// its incoming (operand, label) pairs, and the line it came from.
type PendingPhi = (Value, Vec<(Operand, String)>, usize);

impl<'a> Lowering<'a> {
    /// Lower one `define` as a function of its own in the output module.
    fn standalone(&mut self, name: &str) -> Result<FuncId, LowerError> {
        if let Some(id) = self.out.find(name) {
            return Ok(id);
        }
        let def = self
            .source
            .define(name)
            .ok_or_else(|| LowerError::Malformed {
                what: format!("`@{name}` is not defined in the module"),
                line: 1,
            })?;
        let mut params = Vec::new();
        for p in &def.params {
            let ty = engine_ty(&p.ty).ok_or_else(|| LowerError::Unsupported {
                what: format!(
                    "a parameter of type {} (`%{}` of `@{}`)",
                    p.ty.spelling(),
                    p.name,
                    def.name
                ),
                line: def.line,
            })?;
            params.push(ty);
        }
        if def.ret != Type::Double {
            return Err(LowerError::Unsupported {
                what: format!(
                    "a function returning {} (`@{}`); a function returns double",
                    def.ret.spelling(),
                    def.name
                ),
                line: def.line,
            });
        }
        let mut builder = Builder {
            f: Func::new(name, params, Ty::Real),
            consts: HashMap::new(),
            source: self.source,
            rule_ids: &self.rule_ids,
            rules_applied: &mut self.rules_applied,
        };
        let entry: BlockId = 0;
        let args: Vec<Value> = (0..def.params.len())
            .map(|i| {
                builder
                    .f
                    .push(entry, Op::Param(i as u32), builder.f.params[i])
            })
            .collect();
        let mut stack = vec![name.to_owned()];
        let rets = builder.emit(def, args, entry, &mut stack)?;
        // Every return of the top-level function returns from it.
        for (block, value) in rets {
            builder.f.blocks[block as usize].term = Term::Ret(Some(value));
        }
        Ok(self.out.add(builder.f))
    }
}

impl<'a> Builder<'a> {
    fn const_real(&mut self, x: f64, cur: BlockId) -> Value {
        self.constant(true, x.to_bits(), Op::ConstReal(x), Ty::Real, cur)
    }
    fn const_int(&mut self, i: i64, cur: BlockId) -> Value {
        self.constant(false, i as u64, Op::ConstInt(i), Ty::Int, cur)
    }
    /// A constant, placed once in the entry block. Placed at the front when
    /// the entry block has already been left, so that it is defined before
    /// everything, including a phi that reads it on an edge.
    fn constant(&mut self, real: bool, bits: u64, op: Op, ty: Ty, cur: BlockId) -> Value {
        if let Some(&v) = self.consts.get(&(real, bits)) {
            return v;
        }
        let v = if cur == 0 {
            self.f.push(0, op, ty)
        } else {
            self.f.push_front(0, op, ty)
        };
        self.consts.insert((real, bits), v);
        v
    }

    fn operand(
        &mut self,
        frame: &Frame,
        operand: &Operand,
        cur: BlockId,
        line: usize,
    ) -> Result<Value, LowerError> {
        match operand {
            Operand::Local(name) => {
                frame
                    .env
                    .get(name)
                    .copied()
                    .ok_or_else(|| LowerError::Malformed {
                        what: format!("`%{name}` is used before any definition reaches it"),
                        line,
                    })
            }
            Operand::Real(x) => Ok(self.const_real(*x, cur)),
            Operand::Int(i) => Ok(self.const_int(*i, cur)),
        }
    }

    /// Emit `def`'s body with `args` bound to its parameters, its entry
    /// block's instructions going into `first`. Returns each block that ends
    /// in a `ret`, with the returned value, and leaves those blocks without a
    /// terminator for the caller to decide.
    fn emit(
        &mut self,
        def: &llvm::Function,
        args: Vec<Value>,
        first: BlockId,
        stack: &mut Vec<String>,
    ) -> Result<Vec<(BlockId, Value)>, LowerError> {
        let mut frame = Frame {
            env: HashMap::new(),
            starts: HashMap::new(),
            ends: HashMap::new(),
            phis: Vec::new(),
        };
        for (p, v) in def.params.iter().zip(args) {
            frame.env.insert(p.name.clone(), v);
        }
        let Some(entry) = def.blocks.first() else {
            return Err(LowerError::Malformed {
                what: format!("`@{}` has no body", def.name),
                line: def.line,
            });
        };
        // Only the blocks control flow can reach get a block of their own:
        // an unreachable block would be emitted with no terminator, and every
        // pass over the function would have to know to step around it.
        let order = rpo(def)?;
        frame.starts.insert(entry.label.clone(), first);
        for &index in order.iter().skip(1) {
            let block = &def.blocks[index];
            let id = self.f.new_block(&block.label);
            frame.starts.insert(block.label.clone(), id);
        }
        frame.ends = frame.starts.clone();

        let mut rets = Vec::new();
        for index in order {
            let block = &def.blocks[index];
            let mut cur = frame.starts[&block.label];
            for instruction in &block.insts {
                cur = self.instruction(def, &mut frame, instruction, cur, stack)?;
            }
            frame.ends.insert(block.label.clone(), cur);
            let term = match &block.term {
                Terminator::Br(target) => Term::Br(self.target(&frame, target, block.line)?),
                Terminator::CondBr(cond, yes, no) => {
                    let c = self.operand(&frame, cond, cur, block.line)?;
                    Term::CondBr(
                        c,
                        self.target(&frame, yes, block.line)?,
                        self.target(&frame, no, block.line)?,
                    )
                }
                Terminator::Ret(value) => {
                    let v = self.operand(&frame, value, cur, block.line)?;
                    let v = match self.f.ty(v) {
                        Ty::Real => v,
                        _ => {
                            return Err(LowerError::Unsupported {
                                what: "a return of an integer; a function returns double"
                                    .to_owned(),
                                line: block.line,
                            })
                        }
                    };
                    rets.push((cur, v));
                    Term::Unset
                }
                Terminator::RetVoid => {
                    return Err(LowerError::Unsupported {
                        what: "`ret void`; a function returns double".to_owned(),
                        line: block.line,
                    })
                }
                Terminator::Unsupported(what) => {
                    return Err(LowerError::Unsupported {
                        what: what.clone(),
                        line: block.line,
                    })
                }
            };
            self.f.blocks[cur as usize].term = term;
        }

        // Stitch the phis: every incoming value exists now, and every
        // predecessor's end block is known.
        for (phi, incoming, line) in std::mem::take(&mut frame.phis) {
            let mut built = Vec::new();
            for (operand, label) in incoming {
                let Some(&from) = frame.ends.get(&label) else {
                    // An edge from a block control flow never reaches is an
                    // edge that is never taken; LLVM keeps it in the phi and
                    // this IR has no block to name for it.
                    if def.blocks.iter().any(|b| b.label == label) {
                        continue;
                    }
                    return Err(LowerError::Malformed {
                        what: format!("a phi names the block `{label}`, which does not exist"),
                        line,
                    });
                };
                // A phi's operand is read on the edge, so a constant here is
                // placed in the entry block like any other.
                let value = self.operand(&frame, &operand, u32::MAX, line)?;
                built.push((from, value));
            }
            self.f.insts[phi as usize].op = Op::Phi(built);
        }
        Ok(rets)
    }

    fn target(&self, frame: &Frame, label: &str, line: usize) -> Result<BlockId, LowerError> {
        frame
            .starts
            .get(label)
            .copied()
            .ok_or_else(|| LowerError::Malformed {
                what: format!("a branch names the block `{label}`, which does not exist"),
                line,
            })
    }

    /// Emit one instruction into `cur`; returns the block emission continues
    /// in, which is `cur` unless an inlined call split it.
    fn instruction(
        &mut self,
        def: &llvm::Function,
        frame: &mut Frame,
        instruction: &llvm::Instruction,
        cur: BlockId,
        stack: &mut Vec<String>,
    ) -> Result<BlockId, LowerError> {
        let line = instruction.line;
        let unsupported = |what: String| LowerError::Unsupported { what, line };
        let mut next = cur;
        let value: Option<Value> = match &instruction.inst {
            Inst::FBin(op, a, b) => {
                let a = self.operand(frame, a, cur, line)?;
                let b = self.operand(frame, b, cur, line)?;
                let op = match op {
                    FloatOp::Add => BinOp::Add,
                    FloatOp::Sub => BinOp::Sub,
                    FloatOp::Mul => BinOp::Mul,
                    FloatOp::Div => BinOp::Div,
                };
                Some(self.f.push(cur, Op::Bin(op, a, b), Ty::Real))
            }
            Inst::FNeg(a) => {
                let a = self.operand(frame, a, cur, line)?;
                Some(self.f.push(cur, Op::Un(UnOp::Neg, a), Ty::Real))
            }
            Inst::FCmp(pred, a, b) => {
                let a = self.operand(frame, a, cur, line)?;
                let b = self.operand(frame, b, cur, line)?;
                let simple = match pred.as_str() {
                    "oeq" => Some(Pred::Eq),
                    "olt" => Some(Pred::Lt),
                    "ole" => Some(Pred::Le),
                    "ogt" => Some(Pred::Gt),
                    "oge" => Some(Pred::Ge),
                    "une" => Some(Pred::Ne),
                    _ => None,
                };
                match (simple, pred.as_str()) {
                    (Some(p), _) => Some(self.f.push(cur, Op::FCmp(p, a, b), Ty::Int)),
                    // Ordered and not equal: false on a NaN, which the
                    // engine's `ne` is not, so it is `a < b or a > b`.
                    (None, "one") => {
                        let lt = self.f.push(cur, Op::FCmp(Pred::Lt, a, b), Ty::Int);
                        let gt = self.f.push(cur, Op::FCmp(Pred::Gt, a, b), Ty::Int);
                        Some(self.f.push(cur, Op::Int(IntOp::Or, lt, gt), Ty::Int))
                    }
                    // Unordered: true when either operand is NaN, else the
                    // ordered comparison. rustc writes `fcmp ult` for a `>=`
                    // whose branches it swapped, so this is every loop guard
                    // on a float. The engine's `ne` is true on a NaN, so
                    // `a ne a` is exactly "a is NaN".
                    (None, unordered @ ("ult" | "ule" | "ugt" | "uge" | "ueq")) => {
                        let ordered = match unordered {
                            "ult" => Pred::Lt,
                            "ule" => Pred::Le,
                            "ugt" => Pred::Gt,
                            "uge" => Pred::Ge,
                            _ => Pred::Eq,
                        };
                        let cmp = self.f.push(cur, Op::FCmp(ordered, a, b), Ty::Int);
                        let a_nan = self.f.push(cur, Op::FCmp(Pred::Ne, a, a), Ty::Int);
                        let b_nan = self.f.push(cur, Op::FCmp(Pred::Ne, b, b), Ty::Int);
                        let nan = self.f.push(cur, Op::Int(IntOp::Or, a_nan, b_nan), Ty::Int);
                        Some(self.f.push(cur, Op::Int(IntOp::Or, cmp, nan), Ty::Int))
                    }
                    (None, other) => {
                        return Err(unsupported(format!(
                            "the `fcmp {other}` predicate; the subset has oeq one olt ole ogt oge \
                             une ueq ult ule ugt and uge"
                        )))
                    }
                }
            }
            Inst::IBin(op, a, b) => {
                let a = self.operand(frame, a, cur, line)?;
                let b = self.operand(frame, b, cur, line)?;
                let op = match op {
                    IntegerOp::Add => IntOp::Add,
                    IntegerOp::Sub => IntOp::Sub,
                    IntegerOp::Mul => IntOp::Mul,
                    IntegerOp::SDiv => IntOp::Div,
                    IntegerOp::UDiv => IntOp::UDiv,
                    IntegerOp::SRem => IntOp::Rem,
                    IntegerOp::URem => IntOp::URem,
                    IntegerOp::And => IntOp::And,
                    IntegerOp::Or => IntOp::Or,
                    IntegerOp::Xor => IntOp::Xor,
                    IntegerOp::Shl => IntOp::Shl,
                    IntegerOp::LShr => IntOp::LShr,
                    IntegerOp::AShr => IntOp::Shr,
                };
                Some(self.f.push(cur, Op::Int(op, a, b), Ty::Int))
            }
            Inst::ICmp(pred, a, b) => {
                let a = self.operand(frame, a, cur, line)?;
                let b = self.operand(frame, b, cur, line)?;
                let p = match pred.as_str() {
                    "eq" => Pred::Eq,
                    "ne" => Pred::Ne,
                    "slt" => Pred::Lt,
                    "sle" => Pred::Le,
                    "sgt" => Pred::Gt,
                    "sge" => Pred::Ge,
                    "ult" => Pred::ULt,
                    "ule" => Pred::ULe,
                    "ugt" => Pred::UGt,
                    "uge" => Pred::UGe,
                    other => {
                        return Err(unsupported(format!("the `icmp {other}` predicate")));
                    }
                };
                Some(self.f.push(cur, Op::ICmp(p, a, b), Ty::Int))
            }
            Inst::Select { cond, ty, yes, no } => {
                let c = self.operand(frame, cond, cur, line)?;
                let a = self.operand(frame, yes, cur, line)?;
                let b = self.operand(frame, no, cur, line)?;
                let ty = engine_ty(ty)
                    .ok_or_else(|| unsupported(format!("`select` on type {}", ty.spelling())))?;
                Some(self.f.push(cur, Op::Select(c, a, b), ty))
            }
            Inst::Cast(cast, a, _to) => {
                let a = self.operand(frame, a, cur, line)?;
                Some(match cast {
                    // One integer register, whatever the declared width.
                    Cast::ZExt | Cast::SExt | Cast::Trunc => a,
                    Cast::SIToFP | Cast::UIToFP { nneg: true } => {
                        self.f.push(cur, Op::IntToReal(a), Ty::Real)
                    }
                    Cast::UIToFP { nneg: false } => {
                        // The same bits as an unsigned number: negative as a
                        // signed value means 2^64 further along.
                        let signed = self.f.push(cur, Op::IntToReal(a), Ty::Real);
                        let zero = self.const_int(0, cur);
                        let negative = self.f.push(cur, Op::ICmp(Pred::Lt, a, zero), Ty::Int);
                        let wrap = self.const_real(18_446_744_073_709_551_616.0, cur);
                        let shifted = self
                            .f
                            .push(cur, Op::Bin(BinOp::Add, signed, wrap), Ty::Real);
                        self.f
                            .push(cur, Op::Select(negative, shifted, signed), Ty::Real)
                    }
                    Cast::FPToSI => self.f.push(cur, Op::RealToInt(a), Ty::Int),
                    Cast::FPToUI => {
                        // Values past the signed range wrap the same way.
                        let limit = self.const_real(9_223_372_036_854_775_808.0, cur);
                        let wrap = self.const_real(18_446_744_073_709_551_616.0, cur);
                        let high = self.f.push(cur, Op::FCmp(Pred::Ge, a, limit), Ty::Int);
                        let reduced = self.f.push(cur, Op::Bin(BinOp::Sub, a, wrap), Ty::Real);
                        let direct = self.f.push(cur, Op::RealToInt(a), Ty::Int);
                        let wrapped = self.f.push(cur, Op::RealToInt(reduced), Ty::Int);
                        self.f.push(cur, Op::Select(high, wrapped, direct), Ty::Int)
                    }
                })
            }
            Inst::Phi(ty, incoming) => {
                let ty = engine_ty(ty)
                    .ok_or_else(|| unsupported(format!("`phi` on type {}", ty.spelling())))?;
                let phi = self.f.push(cur, Op::Phi(Vec::new()), ty);
                frame.phis.push((phi, incoming.clone(), line));
                Some(phi)
            }
            Inst::Call { callee, ret, args } => {
                let (value, after) = self.call(def, frame, callee, ret, args, cur, line, stack)?;
                next = after;
                value
            }
            Inst::Ignored => None,
            Inst::Unsupported(what) => return Err(unsupported(what.clone())),
        };
        if let (Some(name), Some(v)) = (&instruction.result, value) {
            frame.env.insert(name.clone(), v);
        }
        Ok(next)
    }

    /// A call: an intrinsic, a libm name, a rule, an inlined definition, or
    /// a refusal.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        def: &llvm::Function,
        frame: &mut Frame,
        callee: &str,
        ret: &Type,
        args: &[(Type, Operand)],
        cur: BlockId,
        line: usize,
        stack: &mut Vec<String>,
    ) -> Result<(Option<Value>, BlockId), LowerError> {
        let unsupported = |what: String| LowerError::Unsupported { what, line };
        let mut values = Vec::new();
        for (ty, operand) in args {
            if engine_ty(ty).is_none() {
                return Err(unsupported(format!(
                    "an argument of type {} in the call to `@{callee}`",
                    ty.spelling()
                )));
            }
            values.push(self.operand(frame, operand, cur, line)?);
        }

        // The engine's own operations, by their intrinsic and libm names.
        let unary = match callee {
            "llvm.sin.f64" | "sin" => Some(UnOp::Sin),
            "llvm.cos.f64" | "cos" => Some(UnOp::Cos),
            "tan" => Some(UnOp::Tan),
            "llvm.exp.f64" | "exp" => Some(UnOp::Exp),
            "llvm.log.f64" | "log" => Some(UnOp::Log),
            "llvm.sqrt.f64" | "sqrt" => Some(UnOp::Sqrt),
            "tanh" => Some(UnOp::Tanh),
            "sinh" => Some(UnOp::Sinh),
            "cosh" => Some(UnOp::Cosh),
            "llvm.fabs.f64" | "fabs" => Some(UnOp::Abs),
            "erf" => Some(UnOp::Erf),
            _ => None,
        };
        let binary = match callee {
            "llvm.pow.f64" | "pow" => Some(BinOp::Pow),
            "atan2" => Some(BinOp::Atan2),
            // `maximumnum`/`minimumnum` are what rustc writes for `f64::max`
            // and `f64::min`; they differ from `maxnum`/`minnum` only on a NaN
            // or a signed zero, where no derivative is being claimed anyway.
            "llvm.maxnum.f64" | "llvm.maximumnum.f64" | "llvm.maximum.f64" | "fmax" => {
                Some(BinOp::Max)
            }
            "llvm.minnum.f64" | "llvm.minimumnum.f64" | "llvm.minimum.f64" | "fmin" => {
                Some(BinOp::Min)
            }
            _ => None,
        };
        let real_args = |n: usize, values: &[Value], f: &Func| -> bool {
            values.len() == n && values.iter().all(|&v| f.ty(v) == Ty::Real)
        };
        if let Some(op) = unary {
            if *ret != Type::Double || !real_args(1, &values, &self.f) {
                return Err(unsupported(format!(
                    "`@{callee}` with a shape other than double -> double"
                )));
            }
            return Ok((Some(self.f.push(cur, Op::Un(op, values[0]), Ty::Real)), cur));
        }
        if let Some(op) = binary {
            if *ret != Type::Double || !real_args(2, &values, &self.f) {
                return Err(unsupported(format!(
                    "`@{callee}` with a shape other than (double, double) -> double"
                )));
            }
            return Ok((
                Some(
                    self.f
                        .push(cur, Op::Bin(op, values[0], values[1]), Ty::Real),
                ),
                cur,
            ));
        }

        // A rule: the call stays a call, and the transforms know its derivative.
        if let Some(&(f, _g)) = self.rule_ids.get(callee) {
            if !real_args(1, &values, &self.f) || *ret != Type::Double {
                return Err(unsupported(format!(
                    "the rule for `@{callee}` applies to a call of one double returning double"
                )));
            }
            if !self.rules_applied.iter().any(|name| name == callee) {
                self.rules_applied.push(callee.to_owned());
            }
            return Ok((Some(self.f.push(cur, Op::Call(f, values), Ty::Real)), cur));
        }

        // A function defined in the module: inlined, so that the derivative
        // passes through it.
        if let Some(target) = self.source.define(callee) {
            if stack.iter().any(|name| name == callee) {
                return Err(unsupported(format!(
                    "recursion: `@{callee}` calls itself, directly or through `@{}`",
                    def.name
                )));
            }
            if target.params.len() != values.len() {
                return Err(LowerError::Malformed {
                    what: format!(
                        "`@{callee}` takes {} argument(s), the call gives {}",
                        target.params.len(),
                        values.len()
                    ),
                    line,
                });
            }
            for (p, &v) in target.params.iter().zip(&values) {
                match engine_ty(&p.ty) {
                    Some(ty) if ty == self.f.ty(v) => {}
                    _ => {
                        return Err(unsupported(format!(
                            "a parameter of type {} (`%{}` of `@{callee}`)",
                            p.ty.spelling(),
                            p.name
                        )))
                    }
                }
            }
            if target.ret != Type::Double || *ret != Type::Double {
                return Err(unsupported(format!(
                    "a call to `@{callee}` returning {}; a function returns double",
                    target.ret.spelling()
                )));
            }
            stack.push(callee.to_owned());
            let rets = self.emit(target, values, cur, stack)?;
            stack.pop();
            return Ok(match rets.len() {
                0 => {
                    return Err(LowerError::Malformed {
                        what: format!("`@{callee}` never returns"),
                        line,
                    })
                }
                // One return: emission simply carries on in the block it
                // returned from, with its value.
                1 => (Some(rets[0].1), rets[0].0),
                // Several: they meet in a continuation block through a phi.
                _ => {
                    let cont = self.f.new_block(&format!("{callee}.ret"));
                    for &(block, _) in &rets {
                        self.f.blocks[block as usize].term = Term::Br(cont);
                    }
                    let phi = self.f.push(cont, Op::Phi(rets.clone()), Ty::Real);
                    (Some(phi), cont)
                }
            });
        }

        // Declared, and not something the engine can see through.
        Err(LowerError::OpaqueCall {
            callee: callee.to_owned(),
            line,
        })
    }
}

/// Reverse post-order over the blocks of a function, by index into
/// `def.blocks`, starting from the entry.
fn rpo(def: &llvm::Function) -> Result<Vec<usize>, LowerError> {
    let index: HashMap<&str, usize> = def
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.as_str(), i))
        .collect();
    let succs = |i: usize| -> Result<Vec<usize>, LowerError> {
        let block = &def.blocks[i];
        let labels: Vec<&str> = match &block.term {
            Terminator::Br(t) => vec![t.as_str()],
            Terminator::CondBr(_, a, b) => vec![a.as_str(), b.as_str()],
            _ => vec![],
        };
        labels
            .into_iter()
            .map(|l| {
                index.get(l).copied().ok_or_else(|| LowerError::Malformed {
                    what: format!("a branch names the block `{l}`, which does not exist"),
                    line: block.line,
                })
            })
            .collect()
    };
    let mut seen = vec![false; def.blocks.len()];
    let mut post = Vec::new();
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some((b, i)) = stack.pop() {
        let s = succs(b)?;
        if i < s.len() {
            stack.push((b, i + 1));
            if !seen[s[i]] {
                seen[s[i]] = true;
                stack.push((s[i], 0));
            }
        } else {
            post.push(b);
        }
    }
    post.reverse();
    Ok(post)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interp::Interp;

    fn lowered(text: &str, function: &str) -> Lowered {
        let module = llvm::parse(text).expect("reads");
        lower(&module, function, &[]).expect("lowers")
    }

    #[test]
    fn a_select_lowers_to_one_block_and_runs() {
        let text = "define double @heat(double %x, double %y) {\n\
                    start:\n\
                    \x20 %c = fcmp ogt double %y, 1.000000e+00\n\
                    \x20 %a = fmul double %y, 8.000000e-01\n\
                    \x20 %s = select i1 %c, double %a, double %y\n\
                    \x20 %e = call double @llvm.exp.f64(double %s)\n\
                    \x20 %r = fmul double %x, %e\n\
                    \x20 ret double %r\n\
                    }\n\
                    declare double @llvm.exp.f64(double)\n";
        let l = lowered(text, "heat");
        assert_eq!(l.module.get(l.id).blocks.len(), 1);
        let mut it = Interp::new(&l.module);
        let v = it.call_real(l.id, &[1.5, 2.0]).expect("runs");
        assert!((v - 1.5 * (1.6f64).exp()).abs() < 1e-12);
    }

    #[test]
    fn an_unordered_compare_and_a_maximumnum_lower_as_rustc_writes_them() {
        // `if since >= 12.0 { a } else { b }` after rustc -O: the compare is
        // `ult` with the arms swapped, and `f64::max` is `llvm.maximumnum`.
        let text = "define double @g(double %since, double %x) {\n\
                    start:\n\
                    \x20 %c = fcmp ult double %since, 1.200000e+01\n\
                    \x20 %m = call double @llvm.maximumnum.f64(double %x, double 0.000000e+00)\n\
                    \x20 %d = fmul double %m, 2.000000e+00\n\
                    \x20 %s = select i1 %c, double %m, double %d\n\
                    \x20 ret double %s\n\
                    }\n\
                    declare double @llvm.maximumnum.f64(double, double)\n";
        let l = lowered(text, "g");
        let mut it = Interp::new(&l.module);
        assert_eq!(it.call_real(l.id, &[11.0, 3.0]).expect("runs"), 3.0);
        assert_eq!(it.call_real(l.id, &[12.0, 3.0]).expect("runs"), 6.0);
        assert_eq!(it.call_real(l.id, &[12.0, -3.0]).expect("runs"), 0.0);
        assert_eq!(it.call_real(l.id, &[f64::NAN, 3.0]).expect("runs"), 3.0);
    }

    #[test]
    fn a_loop_with_a_back_edge_phi_runs_to_its_closed_form() {
        // sum_{i<n} x*i, written the way a compiler writes it: the body last.
        let text = "define double @f(double %x, i64 %n) {\n\
                    start:\n\
                    \x20 %go = icmp sgt i64 %n, 0\n\
                    \x20 br i1 %go, label %loop, label %done\n\
                    done:\n\
                    \x20 %out = phi double [ 0.000000e+00, %start ], [ %acc.next, %loop ]\n\
                    \x20 ret double %out\n\
                    loop:\n\
                    \x20 %acc = phi double [ 0.000000e+00, %start ], [ %acc.next, %loop ]\n\
                    \x20 %i = phi i64 [ 0, %start ], [ %i.next, %loop ]\n\
                    \x20 %fi = sitofp i64 %i to double\n\
                    \x20 %term = fmul double %x, %fi\n\
                    \x20 %acc.next = fadd double %acc, %term\n\
                    \x20 %i.next = add nuw nsw i64 %i, 1\n\
                    \x20 %again = icmp ult i64 %i.next, %n\n\
                    \x20 br i1 %again, label %loop, label %done\n\
                    }\n";
        let l = lowered(text, "f");
        crate::ir::verify(&l.module, l.id).expect("well formed");
        let mut it = Interp::new(&l.module);
        let v = it.call_real(l.id, &[0.5, 5.0]).expect("runs");
        assert_eq!(v, 0.5 * 10.0);
        assert_eq!(it.call_real(l.id, &[0.5, 0.0]).expect("runs"), 0.0);
    }

    #[test]
    fn a_defined_callee_is_inlined_and_a_declared_one_is_opaque() {
        let text = "define double @cube(double %x) {\n\
                    start:\n\
                    \x20 %a = fmul double %x, %x\n\
                    \x20 %b = fmul double %a, %x\n\
                    \x20 ret double %b\n\
                    }\n\
                    define double @f(double %x) {\n\
                    start:\n\
                    \x20 %c = call double @cube(double %x)\n\
                    \x20 %r = fadd double %c, 1.000000e+00\n\
                    \x20 ret double %r\n\
                    }\n\
                    define double @g(double %x) {\n\
                    start:\n\
                    \x20 %s = call double @secret(double %x)\n\
                    \x20 ret double %s\n\
                    }\n\
                    declare double @secret(double)\n";
        let l = lowered(text, "f");
        let f = l.module.get(l.id);
        assert!(!f.insts.iter().any(|i| matches!(i.op, Op::Call(_, _))));
        let mut it = Interp::new(&l.module);
        assert_eq!(it.call_real(l.id, &[2.0]).expect("runs"), 9.0);
        let module = llvm::parse(text).expect("reads");
        assert!(matches!(
            lower(&module, "g", &[]),
            Err(LowerError::OpaqueCall { ref callee, line: 15 }) if callee == "secret"
        ));
    }

    #[test]
    fn a_rule_keeps_the_call_and_records_that_it_applied() {
        let text = "define double @cube(double %x) {\n\
                    start:\n\
                    \x20 %a = fmul double %x, %x\n\
                    \x20 %b = fmul double %a, %x\n\
                    \x20 ret double %b\n\
                    }\n\
                    define double @cube_gradient(double %x) {\n\
                    start:\n\
                    \x20 %a = fmul double %x, %x\n\
                    \x20 %b = fmul double %a, 3.000000e+00\n\
                    \x20 ret double %b\n\
                    }\n\
                    define double @f(double %x) {\n\
                    start:\n\
                    \x20 %c = call double @cube(double %x)\n\
                    \x20 ret double %c\n\
                    }\n";
        let module = llvm::parse(text).expect("reads");
        let rules = [RuleBinding {
            function: "cube".to_owned(),
            derivative: "cube_gradient".to_owned(),
        }];
        let l = lower(&module, "f", &rules).expect("lowers");
        assert_eq!(l.rules_applied.len(), 1);
        assert_eq!(l.rules_applied[0].0, "cube");
        assert!(l
            .module
            .get(l.id)
            .insts
            .iter()
            .any(|i| matches!(i.op, Op::Call(_, _))));
    }

    #[test]
    fn recursion_and_memory_are_refused_with_a_line() {
        let text = "define double @r(double %x) {\n\
                    start:\n\
                    \x20 %c = call double @r(double %x)\n\
                    \x20 ret double %c\n\
                    }\n";
        let module = llvm::parse(text).expect("reads");
        assert!(matches!(
            lower(&module, "r", &[]),
            Err(LowerError::Unsupported { line: 3, .. })
        ));
        let text = "define double @m(ptr %a) {\n\
                    start:\n\
                    \x20 %v = load double, ptr %a\n\
                    \x20 ret double %v\n\
                    }\n";
        let module = llvm::parse(text).expect("reads");
        assert!(matches!(
            lower(&module, "m", &[]),
            Err(LowerError::Unsupported { ref what, line: 1 }) if what.contains("ptr")
        ));
    }
}
