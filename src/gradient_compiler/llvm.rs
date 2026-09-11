//! The reader for textual LLVM IR: what `rustc --emit=llvm-ir` and
//! `clang -S -emit-llvm` write.
//!
//! `docs/interface.md` §12.3 states the subset. This file reads the *whole*
//! module -- every `define`, every `declare`, the ident and the target triple
//! -- and records, instruction by instruction, either what an instruction is
//! or what put it outside the subset and on which line. It does not refuse
//! anything by itself except text that is not LLVM IR at all: a module that
//! defines `dot` over pointers and `heat` over doubles is a module in which
//! `heat` is differentiable, and the refusal for `dot` arrives when `dot` is
//! asked for, naming the pointer and its line.
//!
//! # What is read and what is skipped
//!
//! An optimising compiler decorates everything: `add nuw nsw`, `uitofp nneg`,
//! `or disjoint`, `fadd fast`, `tail call noundef double @f(double noundef
//! %x) #2`, `!llvm.loop !4` on the end of a branch, `align 8` on a load,
//! attribute groups and metadata lines. None of it changes what the arithmetic
//! is, so the reader drops it: flag words between the opcode and the type,
//! attribute words around a parameter, the `#n` group, anything from the
//! first `!` token on. What is left is the opcode, the types, the operands and
//! the names, which is what the lowering needs.
//!
//! # Line numbers
//!
//! Every instruction, block and function keeps the one-based line it came
//! from, because a refusal that says "getelementptr on line 36" is something a
//! person can open the file and look at, and one that says "getelementptr" is
//! a search.

use std::collections::HashMap;

/// Why the text could not be read as a module at all. The subset is a
/// separate question, answered per instruction by [`Inst::Unsupported`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: usize,
    pub what: String,
}

/// A scalar type in the subset, or something outside it kept by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Double,
    /// `i1`, `i8`, `i16`, `i32`, `i64`: the width is kept for the record and
    /// for `i1`, whose values are 0 and 1.
    Int(u32),
    Void,
    /// A type the subset does not have -- `ptr`, `float`, a vector, a struct
    /// -- with its spelling, so a refusal can name it.
    Other(String),
}

impl Type {
    fn read(word: &str) -> Type {
        match word {
            "double" => Type::Double,
            "void" => Type::Void,
            "i1" => Type::Int(1),
            "i8" => Type::Int(8),
            "i16" => Type::Int(16),
            "i32" => Type::Int(32),
            "i64" => Type::Int(64),
            other => Type::Other(other.to_owned()),
        }
    }

    /// Whether a word begins a type at all, in the positions this reader
    /// expects one.
    fn is_type_word(word: &str) -> bool {
        matches!(
            word,
            "double" | "void" | "float" | "half" | "fp128" | "x86_fp80" | "ptr" | "label"
        ) || (word.starts_with('i')
            && word[1..].chars().all(|c| c.is_ascii_digit())
            && word.len() > 1)
            || word.starts_with('<')
            || word.starts_with('[')
            || word.starts_with('{')
            || word.starts_with('%')
    }

    pub fn spelling(&self) -> String {
        match self {
            Type::Double => "double".to_owned(),
            Type::Int(bits) => format!("i{bits}"),
            Type::Void => "void".to_owned(),
            Type::Other(text) => text.clone(),
        }
    }
}

/// An operand: a local name, or a constant of the type the instruction gave.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Local(String),
    Real(f64),
    Int(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegerOp {
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    URem,
    And,
    Or,
    Xor,
    Shl,
    LShr,
    AShr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cast {
    ZExt,
    SExt,
    Trunc,
    SIToFP,
    /// `nneg` is LLVM's promise that the integer is not negative, in which
    /// case the unsigned conversion is the signed one.
    UIToFP {
        nneg: bool,
    },
    FPToSI,
    FPToUI,
}

/// One instruction, read. `Unsupported` is an instruction too: it is what the
/// lowering refuses, with the line, when it reaches it.
#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    FBin(FloatOp, Operand, Operand),
    FNeg(Operand),
    /// The predicate is kept as its LLVM spelling; the lowering maps the ones
    /// in the subset and refuses the rest by name.
    FCmp(String, Operand, Operand),
    IBin(IntegerOp, Operand, Operand),
    ICmp(String, Operand, Operand),
    Select {
        cond: Operand,
        ty: Type,
        yes: Operand,
        no: Operand,
    },
    Cast(Cast, Operand, Type),
    Phi(Type, Vec<(Operand, String)>),
    Call {
        callee: String,
        ret: Type,
        args: Vec<(Type, Operand)>,
    },
    /// An intrinsic the subset accepts and ignores: `llvm.assume` and its
    /// kind. Nothing is computed.
    Ignored,
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    Br(String),
    CondBr(Operand, String, String),
    Ret(Operand),
    RetVoid,
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub result: Option<String>,
    pub inst: Inst,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub label: String,
    pub insts: Vec<Instruction>,
    pub term: Terminator,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub ret: Type,
    pub params: Vec<Param>,
    pub blocks: Vec<Block>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Declaration {
    pub name: String,
    pub ret: Type,
    pub params: Vec<Type>,
    pub line: usize,
}

/// A module, read.
#[derive(Clone, Debug, Default)]
pub struct Module {
    pub defines: Vec<Function>,
    pub declares: Vec<Declaration>,
    /// The string inside `!llvm.ident`, when the module has one.
    pub ident: Option<String>,
    pub triple: Option<String>,
}

impl Module {
    pub fn define(&self, name: &str) -> Option<&Function> {
        self.defines.iter().find(|f| f.name == name)
    }
    pub fn declare(&self, name: &str) -> Option<&Declaration> {
        self.declares.iter().find(|d| d.name == name)
    }
}

/// Read a whole module.
pub fn parse(text: &str) -> Result<Module, SyntaxError> {
    let mut module = Module::default();
    let mut metadata: HashMap<String, String> = HashMap::new();
    let mut ident_node: Option<String> = None;
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let number = i + 1;
        let line = strip_comment(lines[i]).trim();
        if line.is_empty() {
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("target triple") {
            module.triple = quoted(rest);
        } else if line.starts_with("!llvm.ident") {
            // `!llvm.ident = !{!3}`: the node that holds the string.
            ident_node = line
                .split('{')
                .nth(1)
                .and_then(|inner| inner.split('}').next())
                .map(|node| node.trim().to_owned());
        } else if line.starts_with('!') {
            // `!3 = !{!"rustc version …"}`: a metadata node, kept by name in
            // case the ident points at it.
            if let Some((name, value)) = line.split_once('=') {
                metadata.insert(name.trim().to_owned(), value.trim().to_owned());
            }
        } else if line.starts_with("declare ") {
            module.declares.push(declaration(line, number)?);
        } else if line.starts_with("define ") {
            let (function, next) = definition(&lines, i)?;
            module.defines.push(function);
            i = next;
            continue;
        }
        // `source_filename`, `target datalayout`, `attributes #n`, `@global`
        // and anything else at the top level is not an instruction and is
        // passed over.
        i += 1;
    }
    if let Some(node) = ident_node {
        module.ident = metadata
            .get(&node)
            .and_then(|value| quoted(value))
            .filter(|s| !s.is_empty());
    }
    if module.defines.is_empty() && module.declares.is_empty() {
        return Err(SyntaxError {
            line: 1,
            what: "the text defines or declares no function, so it is not an LLVM module"
                .to_owned(),
        });
    }
    Ok(module)
}

/// The text of a line up to a `;` that is not inside a string.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_string = !in_string,
            ';' if !in_string => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The first double-quoted string on a line, unescaped as far as LLVM's
/// `\XX` escapes go.
fn quoted(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let end = text[start..].find('"')? + start;
    let raw = &text[start..end];
    let mut out = String::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 2 < bytes.len() + 1 {
            if let Some(hex) = raw.get(i + 1..i + 3) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(char::from(byte));
                    i += 3;
                    continue;
                }
            }
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// tokens
// ---------------------------------------------------------------------------

/// A line broken into words. Names keep their sigil (`%x`, `@f`, `!4`),
/// punctuation is one token each, and a quoted string is one token.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            ',' | '(' | ')' | '[' | ']' | '=' | '{' | '}' | '<' | '>' | '*' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                out.push(c.to_string());
            }
            '"' => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                let mut text = String::from("\"");
                for d in chars.by_ref() {
                    text.push(d);
                    if d == '"' {
                        break;
                    }
                }
                out.push(text);
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Everything from the first metadata token on, and every `#n` attribute
/// group, dropped. Both only ever trail the instruction they annotate.
fn strip_trailing(mut toks: Vec<String>) -> Vec<String> {
    if let Some(at) = toks.iter().position(|t| t.starts_with('!')) {
        toks.truncate(at);
        while toks.last().is_some_and(|t| t == ",") {
            toks.pop();
        }
    }
    toks.retain(|t| !(t.starts_with('#') && t[1..].chars().all(|c| c.is_ascii_digit())));
    toks
}

fn is_local(t: &str) -> bool {
    t.starts_with('%')
}

/// Words LLVM puts between an opcode and its type, or around a parameter,
/// that say nothing about the arithmetic.
const FLAG_WORDS: &[&str] = &[
    "nuw", "nsw", "exact", "disjoint", "nneg", "fast", "nnan", "ninf", "nsz", "arcp", "contract",
    "afn", "reassoc", "inbounds", "volatile", "samesign",
];

/// Parameter and return attributes.
fn is_attribute(word: &str) -> bool {
    matches!(
        word,
        "noundef"
            | "nonnull"
            | "readonly"
            | "readnone"
            | "writeonly"
            | "nocapture"
            | "noalias"
            | "signext"
            | "zeroext"
            | "inreg"
            | "returned"
            | "nofree"
            | "dead_on_unwind"
            | "immarg"
            | "range"
            | "captures"
            | "align"
            | "dereferenceable"
            | "dereferenceable_or_null"
            | "byval"
            | "sret"
            | "unnamed_addr"
            | "local_unnamed_addr"
            | "dso_local"
            | "dso_preemptable"
            | "internal"
            | "private"
            | "external"
            | "linkonce"
            | "linkonce_odr"
            | "weak"
            | "weak_odr"
            | "hidden"
            | "protected"
            | "default"
            | "ccc"
            | "fastcc"
            | "coldcc"
            | "comdat"
            | "prefix"
            | "prologue"
            | "personality"
            | "section"
            | "partition"
            | "gc"
            | "nocallback"
            | "nosync"
            | "nounwind"
            | "willreturn"
            | "mustprogress"
            | "norecurse"
            | "speculatable"
            | "memory"
            | "uwtable"
            | "nonlazybind"
            | "cold"
            | "hot"
            | "noinline"
            | "alwaysinline"
            | "optnone"
            | "noreturn"
            | "convergent"
    )
}

// ---------------------------------------------------------------------------
// functions
// ---------------------------------------------------------------------------

/// A cursor over the tokens of one line.
struct Cursor<'a> {
    toks: &'a [String],
    at: usize,
    line: usize,
}

impl<'a> Cursor<'a> {
    fn new(toks: &'a [String], line: usize) -> Self {
        Cursor { toks, at: 0, line }
    }
    fn peek(&self) -> Option<&'a str> {
        self.toks.get(self.at).map(String::as_str)
    }
    fn next(&mut self) -> Option<&'a str> {
        let t = self.peek();
        self.at += 1;
        t
    }
    fn expect(&mut self, want: &str) -> Result<(), SyntaxError> {
        match self.next() {
            Some(t) if t == want => Ok(()),
            Some(t) => Err(self.err(format!("expected `{want}`, found `{}`", short(t)))),
            None => Err(self.err(format!("expected `{want}`, found the end of the line"))),
        }
    }
    fn err(&self, what: String) -> SyntaxError {
        SyntaxError {
            line: self.line,
            what,
        }
    }
    fn done(&self) -> bool {
        self.at >= self.toks.len()
    }

    /// Skip flag and attribute words, and the parenthesised argument of an
    /// attribute such as `captures(none)` or `range(i64 0, 8)`.
    fn skip_decorations(&mut self) {
        while let Some(t) = self.peek() {
            if FLAG_WORDS.contains(&t) || is_attribute(t) {
                self.at += 1;
                if self.peek() == Some("(") {
                    self.skip_parens();
                } else if t == "align" || t == "dereferenceable" {
                    // `align 8`: the number follows without parentheses.
                    if self
                        .peek()
                        .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
                    {
                        self.at += 1;
                    }
                }
                continue;
            }
            // A quoted attribute such as "target-cpu"="x86-64".
            if t.starts_with('"') {
                self.at += 1;
                if self.peek() == Some("=") {
                    self.at += 2;
                }
                continue;
            }
            break;
        }
    }

    fn skip_parens(&mut self) {
        let mut depth = 0usize;
        while let Some(t) = self.next() {
            match t {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// A type, taken as one word, with an aggregate or vector spelled out to
    /// its closing bracket so that it can at least be named.
    fn ty(&mut self) -> Result<Type, SyntaxError> {
        let Some(first) = self.next() else {
            return Err(self.err("expected a type, found the end of the line".to_owned()));
        };
        if first == "<" || first == "[" || first == "{" {
            let close = match first {
                "<" => ">",
                "[" => "]",
                _ => "}",
            };
            let mut text = first.to_owned();
            let mut depth = 1;
            while let Some(t) = self.next() {
                text.push_str(t);
                text.push(' ');
                if t == first {
                    depth += 1;
                } else if t == close {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            return Ok(Type::Other(text.trim().to_owned()));
        }
        if first == "ptr" || first == "float" || first == "half" || first == "fp128" {
            return Ok(Type::Other(first.to_owned()));
        }
        if !Type::is_type_word(first) {
            return Err(self.err(format!("expected a type, found `{}`", short(first))));
        }
        Ok(Type::read(first))
    }

    /// An operand of a known type: a local name or a constant.
    fn operand(&mut self, ty: &Type) -> Result<Operand, SyntaxError> {
        let Some(t) = self.next() else {
            return Err(self.err("expected an operand, found the end of the line".to_owned()));
        };
        if is_local(t) {
            return Ok(Operand::Local(t[1..].to_owned()));
        }
        match ty {
            Type::Double => real_constant(t)
                .map(Operand::Real)
                .ok_or_else(|| self.err(format!("`{}` is not a real constant", short(t)))),
            Type::Int(bits) => int_constant(t, *bits)
                .map(Operand::Int)
                .ok_or_else(|| self.err(format!("`{}` is not an integer constant", short(t)))),
            _ => Ok(Operand::Local(t.to_owned())),
        }
    }

    fn label_operand(&mut self) -> Result<String, SyntaxError> {
        self.expect("label")?;
        match self.next() {
            Some(t) if is_local(t) => Ok(t[1..].to_owned()),
            Some(t) => Err(self.err(format!("expected a block label, found `{}`", short(t)))),
            None => Err(self.err("expected a block label, found the end of the line".to_owned())),
        }
    }
}

/// A decimal like `8.000000e-01`, or the raw bit pattern `0x3FE999…`.
fn real_constant(t: &str) -> Option<f64> {
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        if hex.starts_with(|c: char| c.is_ascii_alphabetic() && !c.is_ascii_hexdigit()) {
            // `0xK…`, `0xL…`, `0xH…`: other float widths' bit patterns.
            return None;
        }
        return u64::from_str_radix(hex, 16).ok().map(f64::from_bits);
    }
    t.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite() || t.contains("inf") || t.contains("nan"))
}

fn int_constant(t: &str, bits: u32) -> Option<i64> {
    match t {
        "true" if bits == 1 => Some(1),
        "false" if bits == 1 => Some(0),
        _ => t
            .parse::<i64>()
            .ok()
            .or_else(|| t.parse::<u64>().ok().map(|u| u as i64)),
    }
}

/// A token in a form a message can carry.
fn short(t: &str) -> String {
    let mut out: String = t.chars().filter(|c| !c.is_control()).take(40).collect();
    if t.chars().count() > 40 {
        out.push('…');
    }
    out
}

/// `declare double @llvm.sin.f64(double) #2`.
fn declaration(line: &str, number: usize) -> Result<Declaration, SyntaxError> {
    let toks = strip_trailing(tokens(line));
    let mut c = Cursor::new(&toks, number);
    c.expect("declare")?;
    c.skip_decorations();
    let ret = c.ty()?;
    c.skip_decorations();
    let name = match c.next() {
        Some(t) if t.starts_with('@') => t[1..].to_owned(),
        Some(t) => return Err(c.err(format!("expected a function name, found `{}`", short(t)))),
        None => return Err(c.err("expected a function name".to_owned())),
    };
    let params = parameters(&mut c)?.into_iter().map(|p| p.ty).collect();
    Ok(Declaration {
        name,
        ret,
        params,
        line: number,
    })
}

/// The parenthesised parameter list of a `define` or `declare`.
fn parameters(c: &mut Cursor<'_>) -> Result<Vec<Param>, SyntaxError> {
    c.expect("(")?;
    let mut params = Vec::new();
    loop {
        match c.peek() {
            Some(")") => {
                c.at += 1;
                break;
            }
            Some("...") => {
                c.at += 1;
                params.push(Param {
                    name: String::new(),
                    ty: Type::Other("...".to_owned()),
                });
            }
            Some(_) => {
                c.skip_decorations();
                let ty = c.ty()?;
                c.skip_decorations();
                let name = match c.peek() {
                    Some(t) if is_local(t) => {
                        c.at += 1;
                        t[1..].to_owned()
                    }
                    // An unnamed parameter: LLVM numbers it with the other
                    // unnamed values, from zero.
                    _ => params.len().to_string(),
                };
                params.push(Param { name, ty });
            }
            None => return Err(c.err("the parameter list is not closed".to_owned())),
        }
        match c.peek() {
            Some(",") => c.at += 1,
            Some(")") => {}
            Some(t) => {
                return Err(c.err(format!(
                    "expected `,` or `)` in the parameter list, found `{}`",
                    short(t)
                )))
            }
            None => return Err(c.err("the parameter list is not closed".to_owned())),
        }
    }
    Ok(params)
}

/// A `define` from its first line to its closing brace. Returns the function
/// and the index of the line after the brace.
fn definition(lines: &[&str], start: usize) -> Result<(Function, usize), SyntaxError> {
    let number = start + 1;
    let head = strip_comment(lines[start]);
    let toks = strip_trailing(tokens(head));
    let mut c = Cursor::new(&toks, number);
    c.expect("define")?;
    c.skip_decorations();
    let ret = c.ty()?;
    c.skip_decorations();
    let name = match c.next() {
        Some(t) if t.starts_with('@') => t[1..].to_owned(),
        Some(t) => return Err(c.err(format!("expected a function name, found `{}`", short(t)))),
        None => return Err(c.err("expected a function name".to_owned())),
    };
    let params = parameters(&mut c)?;
    c.skip_decorations();
    if c.peek() != Some("{") {
        return Err(c.err("expected `{` to open the function body".to_owned()));
    }

    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<(String, Vec<Instruction>, usize)> = None;
    let mut i = start + 1;
    // The first block may have no label at all, in which case LLVM calls it
    // by the next unnamed number -- which is the parameter count for a
    // function whose parameters are all unnamed, and zero otherwise. Only
    // its identity matters here, so a name no source label can collide with.
    let mut implicit_entry = true;
    while i < lines.len() {
        let line_number = i + 1;
        let raw = strip_comment(lines[i]);
        let line = raw.trim();
        i += 1;
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            if let Some((label, insts, at)) = current.take() {
                return Err(SyntaxError {
                    line: at,
                    what: format!(
                        "block `{}` with {} instruction(s) has no terminator",
                        short(&label),
                        insts.len()
                    ),
                });
            }
            let function = Function {
                name,
                ret,
                params,
                blocks,
                line: number,
            };
            return Ok((function, i));
        }
        // A label: `bb2.preheader:` at the start of the line.
        if !raw.starts_with(' ') && !raw.starts_with('\t') {
            if let Some(label) = line.strip_suffix(':').filter(|l| !l.contains(' ')) {
                if let Some((label, insts, at)) = current.take() {
                    return Err(SyntaxError {
                        line: at,
                        what: format!(
                            "block `{}` with {} instruction(s) has no terminator",
                            short(&label),
                            insts.len()
                        ),
                    });
                }
                current = Some((label.to_owned(), Vec::new(), line_number));
                implicit_entry = false;
                continue;
            }
        }
        if current.is_none() {
            if !implicit_entry {
                return Err(SyntaxError {
                    line: line_number,
                    what: "an instruction outside any block".to_owned(),
                });
            }
            current = Some(("entry".to_owned(), Vec::new(), line_number));
        }
        let toks = strip_trailing(tokens(line));
        match statement(&toks, line_number)? {
            Statement::Inst(instruction) => {
                if let Some((_, insts, _)) = current.as_mut() {
                    insts.push(instruction);
                }
            }
            Statement::Term(term) => {
                let (label, insts, at) = current.take().expect("a block is open");
                blocks.push(Block {
                    label,
                    insts,
                    term,
                    line: at,
                });
            }
        }
    }
    Err(SyntaxError {
        line: number,
        what: format!("the body of `@{}` is never closed", short(&name)),
    })
}

enum Statement {
    Inst(Instruction),
    Term(Terminator),
}

/// One line inside a function body.
fn statement(toks: &[String], line: usize) -> Result<Statement, SyntaxError> {
    let mut c = Cursor::new(toks, line);
    let result = if toks.len() >= 2 && is_local(&toks[0]) && toks[1] == "=" {
        c.at = 2;
        Some(toks[0][1..].to_owned())
    } else {
        None
    };
    let Some(opcode) = c.next() else {
        return Err(c.err("an empty statement".to_owned()));
    };
    let inst = match opcode {
        "br" => {
            let term = if c.peek() == Some("label") {
                Terminator::Br(c.label_operand()?)
            } else {
                let ty = c.ty()?;
                let cond = c.operand(&ty)?;
                c.expect(",")?;
                let yes = c.label_operand()?;
                c.expect(",")?;
                let no = c.label_operand()?;
                Terminator::CondBr(cond, yes, no)
            };
            return Ok(Statement::Term(term));
        }
        "ret" => {
            let ty = c.ty()?;
            let term = match ty {
                Type::Void => Terminator::RetVoid,
                Type::Double | Type::Int(_) => Terminator::Ret(c.operand(&ty)?),
                Type::Other(what) => Terminator::Unsupported(format!("a return of type {what}")),
            };
            return Ok(Statement::Term(term));
        }
        "switch" | "indirectbr" | "invoke" | "unreachable" | "resume" | "callbr" => {
            return Ok(Statement::Term(Terminator::Unsupported(format!(
                "the `{opcode}` terminator"
            ))));
        }
        "fadd" | "fsub" | "fmul" | "fdiv" => {
            let op = match opcode {
                "fadd" => FloatOp::Add,
                "fsub" => FloatOp::Sub,
                "fmul" => FloatOp::Mul,
                _ => FloatOp::Div,
            };
            c.skip_decorations();
            let ty = c.ty()?;
            let a = c.operand(&ty)?;
            c.expect(",")?;
            let b = c.operand(&ty)?;
            match ty {
                Type::Double => Inst::FBin(op, a, b),
                other => Inst::Unsupported(format!("`{opcode}` on type {}", other.spelling())),
            }
        }
        "fneg" => {
            c.skip_decorations();
            let ty = c.ty()?;
            let a = c.operand(&ty)?;
            match ty {
                Type::Double => Inst::FNeg(a),
                other => Inst::Unsupported(format!("`fneg` on type {}", other.spelling())),
            }
        }
        "frem" => Inst::Unsupported("the `frem` instruction".to_owned()),
        "add" | "sub" | "mul" | "sdiv" | "udiv" | "srem" | "urem" | "and" | "or" | "xor"
        | "shl" | "lshr" | "ashr" => {
            let op = match opcode {
                "add" => IntegerOp::Add,
                "sub" => IntegerOp::Sub,
                "mul" => IntegerOp::Mul,
                "sdiv" => IntegerOp::SDiv,
                "udiv" => IntegerOp::UDiv,
                "srem" => IntegerOp::SRem,
                "urem" => IntegerOp::URem,
                "and" => IntegerOp::And,
                "or" => IntegerOp::Or,
                "xor" => IntegerOp::Xor,
                "shl" => IntegerOp::Shl,
                "lshr" => IntegerOp::LShr,
                _ => IntegerOp::AShr,
            };
            c.skip_decorations();
            let ty = c.ty()?;
            let a = c.operand(&ty)?;
            c.expect(",")?;
            let b = c.operand(&ty)?;
            match ty {
                Type::Int(_) => Inst::IBin(op, a, b),
                other => Inst::Unsupported(format!("`{opcode}` on type {}", other.spelling())),
            }
        }
        "fcmp" | "icmp" => {
            c.skip_decorations();
            let Some(pred) = c.next() else {
                return Err(c.err(format!("`{opcode}` needs a predicate")));
            };
            let pred = pred.to_owned();
            let ty = c.ty()?;
            let a = c.operand(&ty)?;
            c.expect(",")?;
            let b = c.operand(&ty)?;
            match (opcode, &ty) {
                ("fcmp", Type::Double) => Inst::FCmp(pred, a, b),
                ("icmp", Type::Int(_)) => Inst::ICmp(pred, a, b),
                (_, other) => Inst::Unsupported(format!("`{opcode}` on type {}", other.spelling())),
            }
        }
        "select" => {
            c.skip_decorations();
            let cty = c.ty()?;
            let cond = c.operand(&cty)?;
            c.expect(",")?;
            let ty = c.ty()?;
            let yes = c.operand(&ty)?;
            c.expect(",")?;
            let ty2 = c.ty()?;
            let no = c.operand(&ty2)?;
            match (&cty, &ty) {
                (Type::Int(1), Type::Double | Type::Int(_)) => Inst::Select { cond, ty, yes, no },
                (Type::Int(1), other) => {
                    Inst::Unsupported(format!("`select` on type {}", other.spelling()))
                }
                (other, _) => Inst::Unsupported(format!(
                    "`select` with a condition of type {}",
                    other.spelling()
                )),
            }
        }
        "zext" | "sext" | "trunc" | "sitofp" | "uitofp" | "fptosi" | "fptoui" => {
            let nneg = c.peek() == Some("nneg");
            c.skip_decorations();
            let from = c.ty()?;
            let a = c.operand(&from)?;
            c.expect("to")?;
            let to = c.ty()?;
            let cast = match opcode {
                "zext" => Cast::ZExt,
                "sext" => Cast::SExt,
                "trunc" => Cast::Trunc,
                "sitofp" => Cast::SIToFP,
                "uitofp" => Cast::UIToFP { nneg },
                "fptosi" => Cast::FPToSI,
                _ => Cast::FPToUI,
            };
            let shape_ok = match cast {
                Cast::ZExt | Cast::SExt | Cast::Trunc => {
                    matches!((&from, &to), (Type::Int(_), Type::Int(_)))
                }
                Cast::SIToFP | Cast::UIToFP { .. } => {
                    matches!((&from, &to), (Type::Int(_), Type::Double))
                }
                Cast::FPToSI | Cast::FPToUI => {
                    matches!((&from, &to), (Type::Double, Type::Int(_)))
                }
            };
            if shape_ok {
                Inst::Cast(cast, a, to)
            } else {
                Inst::Unsupported(format!(
                    "`{opcode}` from {} to {}",
                    from.spelling(),
                    to.spelling()
                ))
            }
        }
        "phi" => {
            c.skip_decorations();
            let ty = c.ty()?;
            let mut incoming = Vec::new();
            loop {
                c.expect("[")?;
                let value = c.operand(&ty)?;
                c.expect(",")?;
                let from = match c.next() {
                    Some(t) if is_local(t) => t[1..].to_owned(),
                    _ => return Err(c.err("expected a block label in the phi".to_owned())),
                };
                c.expect("]")?;
                incoming.push((value, from));
                if c.peek() == Some(",") {
                    c.at += 1;
                    continue;
                }
                break;
            }
            match ty {
                Type::Double | Type::Int(_) => Inst::Phi(ty, incoming),
                other => Inst::Unsupported(format!("`phi` on type {}", other.spelling())),
            }
        }
        "tail" | "musttail" | "notail" | "call" => {
            if opcode != "call" {
                c.expect("call")?;
            }
            c.skip_decorations();
            let ret = c.ty()?;
            c.skip_decorations();
            // A function type may precede the name: `call double (double) @f(...)`.
            if c.peek() == Some("(") {
                c.skip_parens();
            }
            let callee = match c.next() {
                Some(t) if t.starts_with('@') => t[1..].to_owned(),
                Some(t) => {
                    return Ok(Statement::Inst(Instruction {
                        result,
                        inst: Inst::Unsupported(format!(
                            "a call through `{}` rather than a named function",
                            short(t)
                        )),
                        line,
                    }))
                }
                None => return Err(c.err("`call` needs a callee".to_owned())),
            };
            c.expect("(")?;
            let mut args = Vec::new();
            while c.peek() != Some(")") {
                if c.done() {
                    return Err(c.err("the argument list is not closed".to_owned()));
                }
                c.skip_decorations();
                let ty = c.ty()?;
                c.skip_decorations();
                let value = c.operand(&ty)?;
                args.push((ty, value));
                if c.peek() == Some(",") {
                    c.at += 1;
                }
            }
            c.at += 1;
            if callee == "llvm.assume"
                || callee.starts_with("llvm.lifetime.")
                || callee.starts_with("llvm.dbg.")
                || callee == "llvm.experimental.noalias.scope.decl"
            {
                Inst::Ignored
            } else {
                Inst::Call { callee, ret, args }
            }
        }
        "alloca" | "load" | "store" | "getelementptr" | "atomicrmw" | "cmpxchg" | "fence"
        | "va_arg" => {
            Inst::Unsupported(format!("the `{opcode}` instruction, which touches memory"))
        }
        "bitcast" | "ptrtoint" | "inttoptr" | "addrspacecast" | "fpext" | "fptrunc"
        | "extractvalue" | "insertvalue" | "extractelement" | "insertelement" | "shufflevector"
        | "landingpad" | "freeze" => Inst::Unsupported(format!("the `{opcode}` instruction")),
        other => {
            return Err(c.err(format!(
                "`{}` is not an instruction this reader knows",
                short(other)
            )))
        }
    };
    if !matches!(inst, Inst::Unsupported(_)) && !c.done() {
        return Err(c.err(format!(
            "unexpected `{}` after the instruction",
            short(c.peek().unwrap_or(""))
        )));
    }
    Ok(Statement::Inst(Instruction { result, inst, line }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAT: &str = "\
target triple = \"x86_64-unknown-linux-gnu\"

define noundef double @heat(double noundef %x, double noundef %y) unnamed_addr #1 {
start:
  %_4 = fcmp ogt double %y, 1.000000e+00
  %0 = fmul double %y, 8.000000e-01
  %adjusted.sroa.0.0 = select i1 %_4, double %0, double %y
  %_5 = tail call double @llvm.exp.f64(double %adjusted.sroa.0.0)
  %_0 = fmul double %x, %_5
  ret double %_0
}

declare double @llvm.exp.f64(double) #2

!llvm.ident = !{!3}
!3 = !{!\"rustc version 1.98.0 (88d9e12ae)\"}
";

    #[test]
    fn a_compiler_written_function_reads_with_its_provenance() {
        let module = parse(HEAT).expect("reads");
        assert_eq!(module.triple.as_deref(), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(
            module.ident.as_deref(),
            Some("rustc version 1.98.0 (88d9e12ae)")
        );
        let heat = module.define("heat").expect("heat");
        assert_eq!(heat.params.len(), 2);
        assert_eq!(heat.params[1].name, "y");
        assert_eq!(heat.blocks.len(), 1);
        assert_eq!(heat.blocks[0].insts.len(), 5);
        assert!(matches!(
            heat.blocks[0].insts[3].inst,
            Inst::Call { ref callee, .. } if callee == "llvm.exp.f64"
        ));
        assert!(matches!(heat.blocks[0].term, Terminator::Ret(Operand::Local(ref v)) if v == "_0"));
        assert_eq!(
            module.declare("llvm.exp.f64").map(|d| d.params.len()),
            Some(1)
        );
    }

    #[test]
    fn flags_attributes_and_metadata_are_passed_over() {
        let text = "define double @f(double %x, i64 %n) {\n\
                    start:\n\
                    \x20 %a = add nuw nsw i64 %n, 1\n\
                    \x20 %b = or disjoint i64 %a, 2\n\
                    \x20 %c = uitofp nneg i64 %b to double\n\
                    \x20 %d = fmul fast double %x, %c\n\
                    \x20 tail call void @llvm.assume(i1 true)\n\
                    \x20 br label %next, !llvm.loop !4\n\
                    next:\n\
                    \x20 ret double %d\n\
                    }\n";
        let module = parse(text).expect("reads");
        let f = module.define("f").expect("f");
        assert_eq!(f.blocks.len(), 2);
        let insts = &f.blocks[0].insts;
        assert!(matches!(
            insts[0].inst,
            Inst::IBin(IntegerOp::Add, _, Operand::Int(1))
        ));
        assert!(matches!(
            insts[1].inst,
            Inst::IBin(IntegerOp::Or, _, Operand::Int(2))
        ));
        assert!(matches!(
            insts[2].inst,
            Inst::Cast(Cast::UIToFP { nneg: true }, _, Type::Double)
        ));
        assert!(matches!(insts[4].inst, Inst::Ignored));
        assert!(matches!(f.blocks[0].term, Terminator::Br(ref l) if l == "next"));
    }

    #[test]
    fn real_constants_read_in_both_spellings() {
        assert_eq!(real_constant("8.000000e-01"), Some(0.8));
        assert_eq!(real_constant("0x3FE999999999999A"), Some(0.8));
        assert_eq!(real_constant("-1.500000e+00"), Some(-1.5));
        assert_eq!(
            int_constant("9223372036854775804", 64),
            Some(9223372036854775804)
        );
        assert_eq!(int_constant("true", 1), Some(1));
    }

    #[test]
    fn a_phi_keeps_every_edge_and_a_name_may_carry_dots_and_dashes() {
        let text = "define double @f(double %x) {\n\
                    start:\n\
                    \x20 br label %bb3.loopexit.unr-lcssa\n\
                    bb3.loopexit.unr-lcssa:\n\
                    \x20 %total.sroa.0.06.epil.init = phi double [ 0.000000e+00, %start ], [ %9, %other ]\n\
                    \x20 ret double %total.sroa.0.06.epil.init\n\
                    other:\n\
                    \x20 %9 = fadd double %x, %x\n\
                    \x20 br label %bb3.loopexit.unr-lcssa\n\
                    }\n";
        let module = parse(text).expect("reads");
        let f = module.define("f").expect("f");
        let Inst::Phi(Type::Double, incoming) = &f.blocks[1].insts[0].inst else {
            panic!("a phi");
        };
        assert_eq!(incoming.len(), 2);
        assert_eq!(incoming[1].1, "other");
        assert_eq!(f.blocks[1].label, "bb3.loopexit.unr-lcssa");
    }

    #[test]
    fn memory_and_pointers_are_read_as_unsupported_with_their_line() {
        let text = "define double @dot(ptr %a, i64 %n) {\n\
                    start:\n\
                    \x20 %p = getelementptr inbounds nuw double, ptr %a, i64 %n\n\
                    \x20 %v = load double, ptr %p, align 8, !noundef !4\n\
                    \x20 ret double %v\n\
                    }\n";
        let module = parse(text).expect("reads: the subset is a later question");
        let f = module.define("dot").expect("dot");
        assert_eq!(f.params[0].ty, Type::Other("ptr".to_owned()));
        assert!(
            matches!(f.blocks[0].insts[0].inst, Inst::Unsupported(ref w) if w.contains("getelementptr"))
        );
        assert_eq!(f.blocks[0].insts[1].line, 4);
    }

    #[test]
    fn text_that_is_not_ir_is_a_syntax_error_with_a_line() {
        let text =
            "define double @f(double %x) {\nstart:\n  %y = fadd double %x,\n  ret double %y\n}\n";
        let error = parse(text).expect_err("refused");
        assert_eq!(error.line, 3);
        let unclosed = "define double @f(double %x) {\nstart:\n  ret double %x\n";
        assert!(parse(unclosed).is_err());
        assert_eq!(
            parse("this is prose").expect_err("refused").line,
            1,
            "text with no function in it is not a module"
        );
    }
}
