//! Type analysis: deciding what the bytes are.
//!
//! [`Op::Load`] moves eight bytes and says nothing about them. That is not a
//! shortcut in this IR, it is the situation Enzyme is actually in: by the time
//! LLVM has finished, pointer element types are gone and everything has been
//! through `i8*` at least once. Before you can differentiate a load you have to
//! know whether it loaded a float that carries a derivative or an index that
//! does not, and getting it wrong does not crash -- it silently produces a
//! gradient that is wrong in one term.
//!
//! # What is inferred
//!
//! Pointers are grouped into **memory objects** by union-find: a `gep` is the
//! same object as its base, and a `phi` or `select` over pointers merges
//! theirs. Each object then gets one primitive element type, inferred to a
//! fixpoint from three directions at once:
//!
//! * what is **stored** into it,
//! * what **consumes** the values loaded out of it,
//! * and what other objects it has been **merged** with.
//!
//! Conflicting evidence yields [`Prim::Conflict`] rather than a guess, and the
//! AD passes refuse to differentiate through a conflicted object. A guess here
//! is a wrong gradient; a refusal is a message.
//!
//! # Why it is load-bearing rather than decorative
//!
//! `erase_load_types` throws away every declared load type in a function and
//! `infer` puts them back. `tests/typeanalysis.rs` runs that round trip over
//! the whole benchmark suite and requires the recovered types to equal the
//! original ones exactly. If the analysis were ornamental that test could not
//! pass.

use crate::ir::*;
use std::collections::HashMap;

/// The element type of a memory object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prim {
    /// No evidence yet.
    Unknown,
    Real,
    Int,
    Ptr,
    /// Evidence pointed two ways. Not differentiable.
    Conflict,
}

impl Prim {
    fn join(self, other: Prim) -> Prim {
        match (self, other) {
            (a, Prim::Unknown) => a,
            (Prim::Unknown, b) => b,
            (a, b) if a == b => a,
            _ => Prim::Conflict,
        }
    }
    fn of(ty: Ty) -> Prim {
        match ty {
            Ty::Real => Prim::Real,
            Ty::Int => Prim::Int,
            Ty::Ptr => Prim::Ptr,
            Ty::Void => Prim::Unknown,
        }
    }
    pub fn to_ty(self) -> Ty {
        match self {
            Prim::Real => Ty::Real,
            Prim::Int => Ty::Int,
            Prim::Ptr => Ty::Ptr,
            _ => Ty::Void,
        }
    }
    pub fn differentiable(self) -> bool {
        self == Prim::Real
    }
}

/// The result of the analysis.
pub struct TypeInfo {
    /// Object id for every pointer-typed value; `u32::MAX` for the rest.
    pub object: Vec<u32>,
    /// Element type per object.
    pub element: Vec<Prim>,
    /// Inferred type of every load.
    pub load_ty: HashMap<Value, Prim>,
}

impl TypeInfo {
    /// The element type behind a pointer value.
    pub fn behind(&self, p: Value) -> Prim {
        let o = self.object[p as usize];
        if o == u32::MAX {
            Prim::Unknown
        } else {
            self.element[o as usize]
        }
    }
    pub fn objects(&self) -> usize {
        self.element.len()
    }
    /// Objects whose evidence contradicted itself. AD refuses on these.
    pub fn conflicts(&self) -> Vec<u32> {
        self.element
            .iter()
            .enumerate()
            .filter(|(_, p)| **p == Prim::Conflict)
            .map(|(i, _)| i as u32)
            .collect()
    }
}

struct Uf {
    parent: Vec<u32>,
}
impl Uf {
    fn new(n: usize) -> Self {
        Uf {
            parent: (0..n as u32).collect(),
        }
    }
    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let g = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = g;
            x = g;
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b as usize] = a;
        }
    }
}

/// Remove every declared load type, leaving the IR in the state real optimised
/// IR is in. Used by the round-trip test, and by the demo that shows the
/// analysis doing something.
pub fn erase_load_types(f: &mut Func) -> HashMap<Value, Ty> {
    let mut was = HashMap::new();
    for v in 0..f.insts.len() as Value {
        if matches!(f.insts[v as usize].op, Op::Load(_)) {
            was.insert(v, f.insts[v as usize].ty);
            f.insts[v as usize].ty = Ty::Void;
        }
    }
    was
}

/// Run the analysis over one function.
///
/// `param_hint` lets the caller state what a pointer parameter points at, which
/// is exactly the information a `Duplicated` argument annotation carries at the
/// boundary. Without a hint the analysis still gets there from the uses in most
/// programs; with one it gets there when the function only *writes* the buffer.
pub fn infer(module: &Module, id: FuncId, param_hint: &HashMap<u32, Prim>) -> TypeInfo {
    let f = module.get(id);
    let n = f.insts.len();
    let mut uf = Uf::new(n);

    // 1. group pointers into objects
    for v in f.live_insts() {
        match f.op(v) {
            Op::Gep(p, _) => uf.union(*p, v),
            Op::Phi(inc) if f.ty(v) == Ty::Ptr => {
                for (_, x) in inc {
                    uf.union(v, *x);
                }
            }
            Op::Select(_, a, b) if f.ty(v) == Ty::Ptr => {
                uf.union(v, *a);
                uf.union(v, *b);
            }
            _ => {}
        }
    }

    // 2. number the objects that pointers actually land in
    let mut object = vec![u32::MAX; n];
    let mut ids: HashMap<u32, u32> = HashMap::new();
    for v in f.live_insts() {
        let is_ptr = f.ty(v) == Ty::Ptr
            || matches!(f.op(v), Op::Alloca(_) | Op::Gep(_, _))
            || matches!(f.op(v), Op::Param(i) if f.params[*i as usize] == Ty::Ptr);
        if is_ptr {
            let root = uf.find(v);
            let next = ids.len() as u32;
            let oid = *ids.entry(root).or_insert(next);
            object[v as usize] = oid;
        }
    }
    let mut element = vec![Prim::Unknown; ids.len()];

    // 3. seed from the caller's hints
    for v in f.live_insts() {
        if let Op::Param(i) = f.op(v) {
            if let Some(p) = param_hint.get(i) {
                let o = object[v as usize];
                if o != u32::MAX {
                    element[o as usize] = element[o as usize].join(*p);
                }
            }
        }
    }

    // 4. fixpoint over stores, over what consumes each load, and over calls
    let mut load_ty: HashMap<Value, Prim> = HashMap::new();
    for _ in 0..16 {
        let before = element.clone();
        let before_loads = load_ty.clone();

        // uses of a load tell you what it was
        let mut evidence: HashMap<Value, Prim> = HashMap::new();
        for v in f.live_insts() {
            let note = |x: Value, p: Prim, ev: &mut HashMap<Value, Prim>| {
                if matches!(f.op(x), Op::Load(_)) {
                    let e = ev.entry(x).or_insert(Prim::Unknown);
                    *e = e.join(p);
                }
            };
            match f.op(v).clone() {
                Op::Un(_, a) => note(a, Prim::Real, &mut evidence),
                Op::Bin(_, a, b) | Op::FCmp(_, a, b) => {
                    note(a, Prim::Real, &mut evidence);
                    note(b, Prim::Real, &mut evidence);
                }
                Op::Int(_, a, b) | Op::ICmp(_, a, b) => {
                    note(a, Prim::Int, &mut evidence);
                    note(b, Prim::Int, &mut evidence);
                }
                Op::Gep(p, o) => {
                    note(p, Prim::Ptr, &mut evidence);
                    note(o, Prim::Int, &mut evidence);
                }
                Op::Load(p) => note(p, Prim::Ptr, &mut evidence),
                Op::IntToReal(a) => note(a, Prim::Int, &mut evidence),
                Op::RealToInt(a) => note(a, Prim::Real, &mut evidence),
                Op::Store(p, _) => note(p, Prim::Ptr, &mut evidence),
                Op::Call(g, args) => {
                    for (i, a) in args.iter().enumerate() {
                        note(*a, Prim::of(module.get(g).params[i]), &mut evidence);
                    }
                }
                Op::Select(_, a, b) => {
                    // A select over loads says the two agree, whatever they are.
                    if let (Some(pa), Some(pb)) = (load_ty.get(&a), load_ty.get(&b)) {
                        let j = pa.join(*pb);
                        note(a, j, &mut evidence);
                        note(b, j, &mut evidence);
                    }
                }
                Op::Phi(inc) => {
                    let mut j = Prim::Unknown;
                    for (_, x) in &inc {
                        if let Some(p) = load_ty.get(x) {
                            j = j.join(*p);
                        }
                        if f.ty(*x) != Ty::Void {
                            j = j.join(Prim::of(f.ty(*x)));
                        }
                    }
                    for (_, x) in &inc {
                        note(*x, j, &mut evidence);
                    }
                }
                _ => {}
            }
        }
        for b in f.rpo() {
            match &f.blocks[b as usize].term {
                Term::Ret(Some(v)) if matches!(f.op(*v), Op::Load(_)) => {
                    let e = evidence.entry(*v).or_insert(Prim::Unknown);
                    *e = e.join(Prim::of(f.ret));
                }
                Term::CondBr(c, _, _) if matches!(f.op(*c), Op::Load(_)) => {
                    let e = evidence.entry(*c).or_insert(Prim::Unknown);
                    *e = e.join(Prim::Int);
                }
                _ => {}
            }
        }

        // stores tell you what the object holds; the object tells you what a
        // load out of it produced
        for v in f.live_insts() {
            match *f.op(v) {
                Op::Store(p, x) => {
                    let o = object[p as usize];
                    if o != u32::MAX {
                        let sp = if f.ty(x) == Ty::Void {
                            *load_ty.get(&x).unwrap_or(&Prim::Unknown)
                        } else {
                            Prim::of(f.ty(x))
                        };
                        element[o as usize] = element[o as usize].join(sp);
                    }
                }
                Op::Load(p) => {
                    let o = object[p as usize];
                    let from_obj = if o != u32::MAX {
                        element[o as usize]
                    } else {
                        Prim::Unknown
                    };
                    let from_use = *evidence.get(&v).unwrap_or(&Prim::Unknown);
                    let joined = from_obj.join(from_use);
                    load_ty.insert(v, joined);
                    if o != u32::MAX {
                        element[o as usize] = element[o as usize].join(from_use);
                    }
                }
                _ => {}
            }
        }

        if element == before && load_ty == before_loads {
            break;
        }
    }

    TypeInfo {
        object,
        element,
        load_ty,
    }
}

/// Write the inferred load types back into the IR.
pub fn apply(module: &mut Module, id: FuncId, info: &TypeInfo) -> usize {
    let f = module.get_mut(id);
    let mut n = 0;
    for (&v, &p) in &info.load_ty {
        let t = p.to_ty();
        if t != Ty::Void && f.insts[v as usize].ty != t {
            f.insts[v as usize].ty = t;
            n += 1;
        }
    }
    n
}
