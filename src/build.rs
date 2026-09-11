//! A builder, so that writing test programs does not drown the point.
//!
//! Every method here is a thin wrapper over [`Func::push`]. The value is that
//! a program under test reads like the program it is, which matters when the
//! question being asked is whether its *derivative* is right.

use crate::ir::*;

pub struct Builder<'f> {
    pub f: &'f mut Func,
    pub at: BlockId,
}

impl<'f> Builder<'f> {
    pub fn new(f: &'f mut Func) -> Self {
        let at = f.entry;
        Builder { f, at }
    }
    pub fn at(&mut self, b: BlockId) -> &mut Self {
        self.at = b;
        self
    }
    pub fn block(&mut self, label: &str) -> BlockId {
        self.f.new_block(label)
    }

    pub fn real(&mut self, c: f64) -> Value {
        self.f.push(self.at, Op::ConstReal(c), Ty::Real)
    }
    pub fn int(&mut self, c: i64) -> Value {
        self.f.push(self.at, Op::ConstInt(c), Ty::Int)
    }
    pub fn param(&mut self, i: u32) -> Value {
        let t = self.f.params[i as usize];
        self.f.push(self.at, Op::Param(i), t)
    }
    pub fn un(&mut self, o: UnOp, a: Value) -> Value {
        self.f.push(self.at, Op::Un(o, a), Ty::Real)
    }
    pub fn bin(&mut self, o: BinOp, a: Value, b: Value) -> Value {
        self.f.push(self.at, Op::Bin(o, a, b), Ty::Real)
    }
    pub fn add(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Add, a, b)
    }
    pub fn sub(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Sub, a, b)
    }
    pub fn mul(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Mul, a, b)
    }
    pub fn div(&mut self, a: Value, b: Value) -> Value {
        self.bin(BinOp::Div, a, b)
    }
    pub fn iop(&mut self, o: IntOp, a: Value, b: Value) -> Value {
        self.f.push(self.at, Op::Int(o, a, b), Ty::Int)
    }
    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        self.iop(IntOp::Add, a, b)
    }
    pub fn fcmp(&mut self, p: Pred, a: Value, b: Value) -> Value {
        self.f.push(self.at, Op::FCmp(p, a, b), Ty::Int)
    }
    pub fn icmp(&mut self, p: Pred, a: Value, b: Value) -> Value {
        self.f.push(self.at, Op::ICmp(p, a, b), Ty::Int)
    }
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        let t = self.f.ty(a);
        self.f.push(self.at, Op::Select(c, a, b), t)
    }
    pub fn alloca(&mut self, n: Value) -> Value {
        self.f.push(self.at, Op::Alloca(n), Ty::Ptr)
    }
    pub fn gep(&mut self, p: Value, o: Value) -> Value {
        self.f.push(self.at, Op::Gep(p, o), Ty::Ptr)
    }
    /// A load has to be told what it is loading, because the instruction
    /// itself does not know. In real use [`crate::typeanalysis`] infers this;
    /// the builder takes it so hand-written tests can be explicit.
    pub fn load(&mut self, p: Value, ty: Ty) -> Value {
        self.f.push(self.at, Op::Load(p), ty)
    }
    pub fn store(&mut self, p: Value, x: Value) -> Value {
        self.f.push(self.at, Op::Store(p, x), Ty::Void)
    }
    pub fn phi(&mut self, inc: Vec<(BlockId, Value)>, ty: Ty) -> Value {
        self.f.push_front(self.at, Op::Phi(inc), ty)
    }
    pub fn call(&mut self, g: FuncId, args: Vec<Value>, ty: Ty) -> Value {
        self.f.push(self.at, Op::Call(g, args), ty)
    }
    pub fn sitofp(&mut self, a: Value) -> Value {
        self.f.push(self.at, Op::IntToReal(a), Ty::Real)
    }

    pub fn br(&mut self, t: BlockId) {
        self.f.blocks[self.at as usize].term = Term::Br(t);
    }
    pub fn condbr(&mut self, c: Value, t: BlockId, e: BlockId) {
        self.f.blocks[self.at as usize].term = Term::CondBr(c, t, e);
    }
    pub fn ret(&mut self, v: Value) {
        self.f.blocks[self.at as usize].term = Term::Ret(Some(v));
    }
    pub fn ret_void(&mut self) {
        self.f.blocks[self.at as usize].term = Term::Ret(None);
    }
}
