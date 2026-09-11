//! A general JSON value: read one, hold it, write it back.
//!
//! # Why this is not [`crate::request_json`]
//!
//! That module reads exactly one request shape -- `source` and `at` -- and
//! reports a byte offset when the text is not that shape. It is deliberately
//! narrow, and the narrowness is the point: it cannot be fed a document it
//! then has to interpret.
//!
//! The command line needs the other thing. A problem file, a fixtures file, a
//! tool request and an AI reply are four different documents, and the reader
//! for them has to hand back a *value* the caller then inspects. So this is a
//! whole reader and a whole writer, and every shape check lives with the
//! command that knows what shape it wanted.
//!
//! Two properties the rest of the crate depends on:
//!
//! * **Object key order is preserved.** Objects are a vector of pairs, not a
//!   map. Exports must be byte-for-byte identical between two runs, and a hash
//!   map's iteration order is exactly the thing that would make them not be.
//! * **Non-finite numbers render as `null`.** JSON has no infinity and no NaN,
//!   and a document nobody can parse is worse than one that says "no number
//!   here". [`crate::api`] already made the same choice for the same reason.

use std::fmt::Write as _;

/// Maximum nesting a document may have before it is refused.
///
/// A recursive reader on a hostile document is a stack overflow, which is a
/// crash rather than a refusal, and this crate refuses. Real problem files
/// nest three deep; two hundred thousand open brackets are not a document.
pub const MAX_DEPTH: usize = 64;

/// One JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Why a document could not be read. The byte offset is carried because a
/// caller pointing at the character beats a caller describing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub at: usize,
    pub what: &'static str,
}

impl Json {
    /// Read one whole document. Trailing text is an error, not an invitation.
    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
            depth: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(p.err("one value and nothing after it"));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(pairs) => Some(pairs),
            _ => None,
        }
    }
    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }

    /// Reach into a nested document: `disturbance.applied.concurrency_peak`.
    ///
    /// A missing member anywhere along the way is `None` rather than a
    /// different kind of answer, because every caller of this is reading a
    /// document some other command wrote and has to cope with an older one.
    pub fn path(&self, path: &str) -> Option<&Json> {
        let mut here = self;
        for part in path.split('.') {
            here = here.get(part)?;
        }
        Some(here)
    }

    pub fn path_number(&self, path: &str) -> Option<f64> {
        self.path(path).and_then(Json::as_f64)
    }

    pub fn path_string(&self, path: &str) -> Option<String> {
        self.path(path).and_then(Json::as_str).map(str::to_owned)
    }

    pub fn path_bool(&self, path: &str) -> Option<bool> {
        self.path(path).and_then(Json::as_bool)
    }

    /// The document as text, compact, in the order it was built.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out);
        out
    }

    fn render_into(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => push_number(*n, out),
            Json::Str(s) => push_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.render_into(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    push_string(k, out);
                    out.push(':');
                    v.render_into(out);
                }
                out.push('}');
            }
        }
    }

    /// The document as text over several lines, two spaces per level. Used for
    /// the files an export leaves behind, which a person reads.
    pub fn render_pretty(&self) -> String {
        let mut out = String::new();
        self.pretty_into(0, &mut out);
        out.push('\n');
        out
    }

    fn pretty_into(&self, indent: usize, out: &mut String) {
        let pad = |n: usize, out: &mut String| {
            for _ in 0..n {
                out.push_str("  ");
            }
        };
        match self {
            Json::Arr(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(indent + 1, out);
                    item.pretty_into(indent + 1, out);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(indent, out);
                out.push(']');
            }
            Json::Obj(pairs) if !pairs.is_empty() => {
                out.push_str("{\n");
                for (i, (k, v)) in pairs.iter().enumerate() {
                    pad(indent + 1, out);
                    push_string(k, out);
                    out.push_str(": ");
                    v.pretty_into(indent + 1, out);
                    if i + 1 < pairs.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(indent, out);
                out.push('}');
            }
            other => other.render_into(out),
        }
    }
}

/// Build an object from pairs, keeping the order they were written.
pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Obj(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect::<Vec<_>>(),
    )
}

/// A string value, for the callers that build documents.
pub fn s(text: &str) -> Json {
    Json::Str(text.to_owned())
}

/// A number a JSON document can carry: non-finite becomes `null` on the way
/// out, which is the same choice [`crate::api`] makes.
fn push_number(x: f64, out: &mut String) {
    if x.is_finite() {
        let _ = write!(out, "{x:?}");
    } else {
        out.push_str("null");
    }
}

/// A `f64` exactly as it will appear in a rendered document. Public because
/// the report generator writes the same numbers into prose, and two spellings
/// of one number in one export would be a defect a reader could see.
pub fn number(x: f64) -> String {
    let mut out = String::new();
    push_number(x, &mut out);
    out
}

/// The same double, kept to `digits` significant decimal digits.
///
/// # Why a document would ever want fewer digits than it has
///
/// Because a measurement shipped with a tolerance must not be written to a
/// precision that tolerance cannot support. An exported fixture declares
/// agreement to a relative 1e-9; writing seventeen significant digits next to
/// that claims sixteen orders more than the export is prepared to defend, and
/// invites a reader to treat the last digits as meaningful when the export's
/// own limitations say they are not.
///
/// The rounding is done by going through the decimal form once, so the result
/// is the double nearest that decimal: deterministic, and identical on every
/// platform.
pub fn to_significant(x: f64, digits: usize) -> f64 {
    if !x.is_finite() || x == 0.0 || digits == 0 {
        return x;
    }
    let decimal = format!("{:.*e}", digits - 1, x);
    decimal.parse().unwrap_or(x)
}

fn push_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, what: &'static str) -> JsonError {
        JsonError { at: self.i, what }
    }
    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn lit(&mut self, word: &str) -> Result<(), JsonError> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(())
        } else {
            Err(self.err("a literal `true`, `false` or `null`"))
        }
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        match self.peek() {
            None => Err(self.err("a value, not the end of the document")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => {
                self.lit("true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.lit("false")?;
                Ok(Json::Bool(false))
            }
            Some(b'n') => {
                self.lit("null")?;
                Ok(Json::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("a value")),
        }
    }

    /// Enter one level of nesting, or refuse. The counter is decremented on
    /// the way out so a wide-but-shallow document is never penalised.
    fn nested<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, JsonError>,
    ) -> Result<T, JsonError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err("nesting no deeper than 64 levels"));
        }
        self.depth += 1;
        let out = parse(self);
        self.depth -= 1;
        out
    }

    fn object(&mut self) -> Result<Json, JsonError> {
        self.nested(|p| {
            p.i += 1; // the `{`
            let mut pairs: Vec<(String, Json)> = Vec::new();
            p.ws();
            if p.peek() == Some(b'}') {
                p.i += 1;
                return Ok(Json::Obj(pairs));
            }
            loop {
                p.ws();
                if p.peek() != Some(b'"') {
                    return Err(p.err("a quoted member name"));
                }
                let key = p.string()?;
                p.ws();
                if p.peek() != Some(b':') {
                    return Err(p.err("a `:` after the member name"));
                }
                p.i += 1;
                p.ws();
                let v = p.value()?;
                // A repeated key is a document whose meaning depends on which
                // one the reader kept, so it is refused rather than resolved.
                if pairs.iter().any(|(k, _)| *k == key) {
                    return Err(p.err("member names that are not repeated"));
                }
                pairs.push((key, v));
                p.ws();
                match p.peek() {
                    Some(b',') => p.i += 1,
                    Some(b'}') => {
                        p.i += 1;
                        return Ok(Json::Obj(pairs));
                    }
                    _ => return Err(p.err("a `,` or a `}`")),
                }
            }
        })
    }

    fn array(&mut self) -> Result<Json, JsonError> {
        self.nested(|p| {
            p.i += 1; // the `[`
            let mut items = Vec::new();
            p.ws();
            if p.peek() == Some(b']') {
                p.i += 1;
                return Ok(Json::Arr(items));
            }
            loop {
                p.ws();
                items.push(p.value()?);
                p.ws();
                match p.peek() {
                    Some(b',') => p.i += 1,
                    Some(b']') => {
                        p.i += 1;
                        return Ok(Json::Arr(items));
                    }
                    _ => return Err(p.err("a `,` or a `]`")),
                }
            }
        })
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| JsonError {
            at: start,
            what: "a number in UTF-8",
        })?;
        let value: f64 = text.parse().map_err(|_| JsonError {
            at: start,
            what: "a number",
        })?;
        // `1e999` parses -- to infinity. The refusal says so without repeating
        // the literal back, because echoing hostile input is how a refusal
        // becomes a way of getting text into somebody else's log.
        if !value.is_finite() {
            return Err(JsonError {
                at: start,
                what: "a finite number",
            });
        }
        Ok(Json::Num(value))
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err("a closing quote"));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.i += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.err("an escape sequence"));
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.escaped_char()?),
                        _ => return Err(self.err("a known escape sequence")),
                    }
                }
                c if c < 0x20 => return Err(self.err("no raw control character in a string")),
                _ => {
                    let rest = std::str::from_utf8(&self.s[self.i..]).map_err(|_| JsonError {
                        at: self.i,
                        what: "valid UTF-8",
                    })?;
                    let ch = rest.chars().next().unwrap_or('\u{FFFD}');
                    self.i += ch.len_utf8();
                    out.push(ch);
                }
            }
        }
    }

    /// One `\uXXXX`, and the low half of a surrogate pair when there is one.
    fn escaped_char(&mut self) -> Result<char, JsonError> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) {
            if !self.s[self.i..].starts_with(b"\\u") {
                return Err(self.err("the second half of a surrogate pair"));
            }
            self.i += 2;
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return Err(self.err("the second half of a surrogate pair"));
            }
            let combined = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
            return char::from_u32(combined).ok_or_else(|| self.err("a Unicode scalar value"));
        }
        char::from_u32(hi).ok_or_else(|| self.err("a Unicode scalar value"))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.s.len() {
            return Err(self.err("four hexadecimal digits"));
        }
        let text = std::str::from_utf8(&self.s[self.i..self.i + 4])
            .map_err(|_| self.err("four hexadecimal digits"))?;
        let value =
            u32::from_str_radix(text, 16).map_err(|_| self.err("four hexadecimal digits"))?;
        self.i += 4;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_key_order_survives_a_round_trip() {
        let text = r#"{"b":1,"a":[true,null,"x"]}"#;
        let value = Json::parse(text).expect("parses");
        assert_eq!(value.render(), r#"{"b":1.0,"a":[true,null,"x"]}"#);
    }

    #[test]
    fn a_non_finite_literal_is_refused_rather_than_read_as_infinity() {
        let error = Json::parse("1e999").expect_err("refused");
        assert_eq!(error.what, "a finite number");
    }

    #[test]
    fn nesting_is_bounded_so_a_hostile_document_cannot_exhaust_the_stack() {
        let text = "[".repeat(200_000);
        let error = Json::parse(&text).expect_err("refused");
        assert_eq!(error.what, "nesting no deeper than 64 levels");
    }

    #[test]
    fn non_finite_numbers_render_as_null() {
        assert_eq!(Json::Num(f64::NAN).render(), "null");
        assert_eq!(Json::Num(f64::INFINITY).render(), "null");
    }
}
