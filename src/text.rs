//! A surface a model can actually write.
//!
//! # Why this exists
//!
//! Enzyme's input is LLVM IR. Catalyst's is [`crate::ir`], which is smaller and
//! kinder but still SSA: numbered values, explicit blocks, phis at the joins.
//! A person writes that through [`crate::build::Builder`], in Rust, in a crate
//! they compile. A language model cannot -- not because it is unable to reason
//! about SSA, but because handing one over means constructing values in a
//! running program, and an agent on the other side of a pipe has no running
//! program to construct them in.
//!
//! So the thing missing was never a smarter API. It was a *format*: something a
//! model can emit as characters, and this crate can read back.
//!
//! ```text
//! func poly(x, y) = x*y + sin(x)
//! ```
//!
//! That is the whole language. It lowers to one straight-line function, which
//! is exactly the shape [`crate::reverse`] differentiates, so anything writable
//! here is differentiable in both modes.
//!
//! # What it deliberately does not have
//!
//! No loops, no branches, no assignment, no memory. Not as a first cut with
//! more to come -- as the definition. A format whose programs are always
//! straight-line is a format whose programs can always be differentiated both
//! ways, and a caller never has to discover that the thing it just wrote is
//! outside what the tool can do. Control flow is available, through
//! [`crate::build::Builder`], to callers who are compiling anyway.
//!
//! # Errors are refusals
//!
//! A syntax error comes back as a [`Refusal`] with a stable code and a byte
//! offset, exactly like a refusal from the transforms. A caller writing a
//! repair loop matches on one kind of thing whether it got the grammar wrong or
//! asked for a derivative that cannot be taken.

use crate::build::Builder;
use crate::ir::{BinOp, Func, FuncId, Module, Ty, UnOp, Value};
use crate::refuse::{Cause, Refusal};

/// Every function name the language defines, with how many arguments it takes.
///
/// Public because a caller generating source needs to know what it may use, and
/// a model given this list writes far fewer unknown names than one left to
/// guess. It is the same list the parser resolves against, so it cannot drift
/// out of date the way a documented list would.
pub fn names() -> Vec<(&'static str, usize)> {
    let mut all: Vec<(&'static str, usize)> = UNARY.iter().map(|(n, _)| (*n, 1)).collect();
    all.extend(BINARY.iter().map(|(n, _)| (*n, 2)));
    all
}

const UNARY: &[(&str, UnOp)] = &[
    ("neg", UnOp::Neg),
    ("sin", UnOp::Sin),
    ("cos", UnOp::Cos),
    ("tan", UnOp::Tan),
    ("exp", UnOp::Exp),
    ("log", UnOp::Log),
    ("sqrt", UnOp::Sqrt),
    ("tanh", UnOp::Tanh),
    ("sinh", UnOp::Sinh),
    ("cosh", UnOp::Cosh),
    ("abs", UnOp::Abs),
    ("recip", UnOp::Recip),
    ("erf", UnOp::Erf),
];

const BINARY: &[(&str, BinOp)] = &[
    ("pow", BinOp::Pow),
    ("max", BinOp::Max),
    ("min", BinOp::Min),
    ("atan2", BinOp::Atan2),
];

/// One parsed function, and the parameter names in the order they were written.
///
/// The names are carried because the caller wrote them and will want to talk
/// about the answer in the same terms: a gradient reported against `x` and `y`
/// is usable, one reported against index 0 and 1 has to be decoded first.
pub struct Parsed {
    pub module: Module,
    pub id: FuncId,
    pub name: String,
    pub params: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Ident(String),
    Number(f64),
    Punct(char),
    End,
}

struct Lexer {
    toks: Vec<(Tok, usize)>,
    at: usize,
    nesting: usize,
}

/// Maximum simultaneously nested parentheses, calls, negations and powers.
/// Flat arithmetic chains do not consume this budget.
pub const MAX_EXPRESSION_NESTING: usize = 64;

impl Lexer {
    fn new(source: &str) -> Result<Self, Refusal> {
        let bytes = source.as_bytes();
        let mut toks = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            // A comment runs to the end of its line. Models produce them
            // unprompted, and refusing one would be a refusal about nothing.
            if c == '#' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            let start = i;
            if c.is_ascii_alphabetic() || c == '_' {
                while i < bytes.len()
                    && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                toks.push((Tok::Ident(source[start..i].to_owned()), start));
            } else if c.is_ascii_digit() || c == '.' {
                while i < bytes.len() && ((bytes[i] as char).is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                // An exponent, and only when it is really one: `e` followed by
                // digits, optionally signed. Without this check `2e` would eat
                // the `e` and then fail to parse, which reads as a number error
                // rather than the unknown name it is.
                if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                    let mut j = i + 1;
                    if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                        j += 1;
                    }
                    if j < bytes.len() && (bytes[j] as char).is_ascii_digit() {
                        i = j;
                        while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text = &source[start..i];
                let value = text.parse::<f64>().map_err(|_| {
                    Refusal::about_job(
                        "<source>",
                        Cause::Syntax {
                            at: start,
                            found: text.to_owned(),
                            expected: "a number".to_owned(),
                        },
                    )
                })?;
                toks.push((Tok::Number(value), start));
            } else if "()+-*/^,=".contains(c) {
                toks.push((Tok::Punct(c), start));
                i += 1;
            } else {
                return Err(Refusal::about_job(
                    "<source>",
                    Cause::Syntax {
                        at: start,
                        found: c.to_string(),
                        expected: "a name, a number, or one of ( ) + - * / ^ , =".to_owned(),
                    },
                ));
            }
        }
        toks.push((Tok::End, source.len()));
        Ok(Lexer {
            toks,
            at: 0,
            nesting: 0,
        })
    }

    fn nested<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, Refusal>,
    ) -> Result<T, Refusal> {
        if self.nesting >= MAX_EXPRESSION_NESTING {
            return Err(self.unexpected("expression nesting at most 64 levels deep"));
        }
        self.nesting += 1;
        let result = parse(self);
        self.nesting -= 1;
        result
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.at].0
    }
    fn offset(&self) -> usize {
        self.toks[self.at].1
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.at].0.clone();
        if self.at + 1 < self.toks.len() {
            self.at += 1;
        }
        t
    }
    fn expect_punct(&mut self, c: char) -> Result<(), Refusal> {
        if *self.peek() == Tok::Punct(c) {
            self.next();
            Ok(())
        } else {
            Err(self.unexpected(&format!("'{c}'")))
        }
    }
    fn unexpected(&self, expected: &str) -> Refusal {
        let found = match self.peek() {
            Tok::Ident(name) => name.clone(),
            Tok::Number(n) => n.to_string(),
            Tok::Punct(c) => c.to_string(),
            Tok::End => "the end of the source".to_owned(),
        };
        Refusal::about_job(
            "<source>",
            Cause::Syntax {
                at: self.offset(),
                found,
                expected: expected.to_owned(),
            },
        )
    }
}

// ---------------------------------------------------------------------------
// Parse and lower, in one pass
// ---------------------------------------------------------------------------

/// Read `func name(a, b) = expression` into a module holding one function.
/// Recursive expressions exceeding [`MAX_EXPRESSION_NESTING`] are refused
/// before descending further, rather than exhausting the host thread's stack.
pub fn parse(source: &str) -> Result<Parsed, Refusal> {
    let mut lex = Lexer::new(source)?;

    match lex.next() {
        Tok::Ident(word) if word == "func" => {}
        _ => {
            lex.at = 0;
            return Err(lex.unexpected("the keyword `func`"));
        }
    }
    let name = match lex.next() {
        Tok::Ident(name) => name,
        _ => {
            lex.at -= 1;
            return Err(lex.unexpected("a function name"));
        }
    };
    lex.expect_punct('(')?;
    let mut params: Vec<String> = Vec::new();
    if *lex.peek() != Tok::Punct(')') {
        loop {
            match lex.next() {
                Tok::Ident(p) => {
                    if params.contains(&p) {
                        lex.at -= 1;
                        return Err(lex.unexpected("a parameter name not already declared"));
                    }
                    params.push(p);
                }
                _ => {
                    lex.at -= 1;
                    return Err(lex.unexpected("a parameter name"));
                }
            }
            if *lex.peek() == Tok::Punct(',') {
                lex.next();
                continue;
            }
            break;
        }
    }
    lex.expect_punct(')')?;
    lex.expect_punct('=')?;

    let mut f = Func::new(&name, vec![Ty::Real; params.len()], Ty::Real);
    let body = {
        let mut b = Builder::new(&mut f);
        // Every parameter is materialised whether the body reads it or not, so
        // a gradient can be reported for it. An unread parameter has derivative
        // zero, which is an answer; leaving it out would make it an absence.
        let bound: Vec<(String, Value)> = params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), b.param(i as u32)))
            .collect();
        let root = expr(&mut lex, &mut b, &bound)?;
        b.ret(root);
        root
    };
    let _ = body;

    if *lex.peek() != Tok::End {
        return Err(lex.unexpected("the end of the source"));
    }

    let mut module = Module::new();
    let id = module.add(f);
    Ok(Parsed {
        module,
        id,
        name,
        params,
    })
}

type Bound = [(String, Value)];

fn expr(lex: &mut Lexer, b: &mut Builder, bound: &Bound) -> Result<Value, Refusal> {
    let mut left = term(lex, b, bound)?;
    loop {
        let op = match lex.peek() {
            Tok::Punct('+') => BinOp::Add,
            Tok::Punct('-') => BinOp::Sub,
            _ => return Ok(left),
        };
        lex.next();
        let right = term(lex, b, bound)?;
        left = b.bin(op, left, right);
    }
}

fn term(lex: &mut Lexer, b: &mut Builder, bound: &Bound) -> Result<Value, Refusal> {
    let mut left = unary(lex, b, bound)?;
    loop {
        let op = match lex.peek() {
            Tok::Punct('*') => BinOp::Mul,
            Tok::Punct('/') => BinOp::Div,
            _ => return Ok(left),
        };
        lex.next();
        let right = unary(lex, b, bound)?;
        left = b.bin(op, left, right);
    }
}

fn unary(lex: &mut Lexer, b: &mut Builder, bound: &Bound) -> Result<Value, Refusal> {
    if *lex.peek() == Tok::Punct('-') {
        lex.next();
        let inner = lex.nested(|lex| unary(lex, b, bound))?;
        return Ok(b.un(UnOp::Neg, inner));
    }
    power(lex, b, bound)
}

/// `^` binds tighter than multiplication and associates to the RIGHT, so
/// `2^3^2` is 512 and not 64. Every notation a caller is likely to be copying
/// from agrees on that, and disagreeing silently would be the worst kind of
/// difference: one that produces a number rather than an error.
fn power(lex: &mut Lexer, b: &mut Builder, bound: &Bound) -> Result<Value, Refusal> {
    let base = atom(lex, b, bound)?;
    if *lex.peek() == Tok::Punct('^') {
        lex.next();
        let exponent = lex.nested(|lex| unary(lex, b, bound))?;
        return Ok(b.bin(BinOp::Pow, base, exponent));
    }
    Ok(base)
}

fn atom(lex: &mut Lexer, b: &mut Builder, bound: &Bound) -> Result<Value, Refusal> {
    match lex.next() {
        Tok::Number(c) => Ok(b.real(c)),
        Tok::Punct('(') => {
            let inner = lex.nested(|lex| expr(lex, b, bound))?;
            lex.expect_punct(')')?;
            Ok(inner)
        }
        Tok::Ident(name) => {
            if *lex.peek() == Tok::Punct('(') {
                lex.next();
                let mut args = Vec::new();
                if *lex.peek() != Tok::Punct(')') {
                    loop {
                        args.push(lex.nested(|lex| expr(lex, b, bound))?);
                        if *lex.peek() == Tok::Punct(',') {
                            lex.next();
                            continue;
                        }
                        break;
                    }
                }
                lex.expect_punct(')')?;
                return call(lex, b, &name, &args);
            }
            bound
                .iter()
                .find(|(p, _)| *p == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| {
                    Refusal::about_job("<source>", Cause::UnknownName { name: name.clone() })
                })
        }
        _ => {
            lex.at -= 1;
            Err(lex.unexpected("a number, a name, or '('"))
        }
    }
}

fn call(lex: &Lexer, b: &mut Builder, name: &str, args: &[Value]) -> Result<Value, Refusal> {
    let arity = |expected: usize| {
        if args.len() == expected {
            Ok(())
        } else {
            Err(Refusal::about_job(
                "<source>",
                Cause::WrongArity {
                    name: name.to_owned(),
                    expected,
                    got: args.len(),
                },
            ))
        }
    };
    if let Some((_, o)) = UNARY.iter().find(|(n, _)| *n == name) {
        arity(1)?;
        return Ok(b.un(*o, args[0]));
    }
    if let Some((_, o)) = BINARY.iter().find(|(n, _)| *n == name) {
        arity(2)?;
        return Ok(b.bin(*o, args[0], args[1]));
    }
    let _ = lex;
    Err(Refusal::about_job(
        "<source>",
        Cause::UnknownName {
            name: name.to_owned(),
        },
    ))
}
