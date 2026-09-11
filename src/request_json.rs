//! Strict reading of the documented two-field request, without dependencies.
//! Never search inside nested objects or discard malformed coordinates.

use crate::api::MAX_REQUEST_BYTES;

pub(crate) fn parse(input: &str) -> Result<(String, Vec<f64>), usize> {
    if input.len() > MAX_REQUEST_BYTES {
        return Err(0);
    }
    let mut p = Parser { input, at: 0 };
    p.expect(b'{')?;
    let mut source = None;
    let mut coordinates = None;
    loop {
        let key = p.string()?;
        p.expect(b':')?;
        match key.as_str() {
            "source" if source.is_none() => source = Some(p.string()?),
            "at" if coordinates.is_none() => coordinates = Some(p.numbers()?),
            // Unknown and duplicate fields are not part of this request schema.
            _ => return Err(p.at),
        }
        if p.take(b'}') {
            break;
        }
        p.expect(b',')?;
    }
    p.whitespace();
    if p.at != input.len() {
        return Err(p.at);
    }
    Ok((source.ok_or(p.at)?, coordinates.ok_or(p.at)?))
}

struct Parser<'a> {
    input: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.at).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn take(&mut self, byte: u8) -> bool {
        self.whitespace();
        if self.peek() == Some(byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), usize> {
        if self.take(byte) {
            Ok(())
        } else {
            Err(self.at)
        }
    }

    fn hex_quad(&mut self) -> Result<u16, usize> {
        let digits = self.input.get(self.at..self.at + 4).ok_or(self.at)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(self.at);
        }
        let value = u16::from_str_radix(digits, 16).map_err(|_| self.at)?;
        self.at += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, usize> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let ch = self.input[self.at..].chars().next().ok_or(self.at)?;
            self.at += ch.len_utf8();
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escape = self.peek().ok_or(self.at)?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex_quad()?;
                            let code = if (0xd800..=0xdbff).contains(&first) {
                                if self.input.as_bytes().get(self.at..self.at + 2) != Some(b"\\u") {
                                    return Err(self.at);
                                }
                                self.at += 2;
                                let second = self.hex_quad()?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err(self.at);
                                }
                                0x10000
                                    + ((u32::from(first) - 0xd800) << 10)
                                    + (u32::from(second) - 0xdc00)
                            } else {
                                u32::from(first)
                            };
                            out.push(char::from_u32(code).ok_or(self.at)?);
                        }
                        _ => return Err(self.at - 1),
                    }
                }
                c if c < '\u{0020}' => return Err(self.at - c.len_utf8()),
                c => out.push(c),
            }
        }
    }

    fn digits(&mut self) -> Result<(), usize> {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        if self.at == start {
            Err(self.at)
        } else {
            Ok(())
        }
    }

    fn number(&mut self) -> Result<f64, usize> {
        self.whitespace();
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => self.digits()?,
            _ => return Err(self.at),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            self.digits()?;
        }
        let value: f64 = self.input[start..self.at].parse().map_err(|_| start)?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(start)
        }
    }

    fn numbers(&mut self) -> Result<Vec<f64>, usize> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        if self.take(b']') {
            return Ok(values);
        }
        loop {
            values.push(self.number()?);
            if self.take(b']') {
                return Ok(values);
            }
            self.expect(b',')?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_string_escapes_round_trip_without_reinterpreting_content() {
        let (source, at) =
            parse(r#"{"source":"é\"\\\/\b\f\n\r\t\u0041\uD83D\uDE80","at":[-0.5,2E+2]}"#).unwrap();
        assert_eq!(source, "é\"\\/\u{0008}\u{000c}\n\r\tA🚀");
        assert_eq!(at, vec![-0.5, 200.0]);
    }

    #[test]
    fn every_truncated_prefix_is_refused_without_panicking() {
        let input = r#"{"source":"é\uD83D\uDE80","at":[-2.5e-3,4]}"#;
        for (end, _) in input.char_indices() {
            assert!(
                parse(&input[..end]).is_err(),
                "accepted prefix ending at {end}"
            );
        }
        assert!(parse(input).is_ok());
    }

    #[test]
    fn malformed_unicode_and_schema_are_refused() {
        for input in [
            r#"{"source":"\uD800","at":[]}"#,
            r#"{"source":"\uDC00","at":[]}"#,
            r#"{"source":"\uD800\u0041","at":[]}"#,
            r#"{"source":"\u+123","at":[]}"#,
            r#"{"source":"\é","at":[]}"#,
            r#"{"source":"x","\u0073ource":"y","at":[]}"#,
            r#"{"source":"x","at":[],"extra":true}"#,
            r#"{"source":"x","at":[[]]}"#,
            r#"{"source":"x","at":[1e]}"#,
            r#"{"source":"x","at":[.5]}"#,
            r#"{"source":"x","at":[1,,2]}"#,
        ] {
            assert!(parse(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn oversized_requests_are_refused_before_parsing() {
        assert_eq!(parse(&" ".repeat(MAX_REQUEST_BYTES + 1)), Err(0));
    }
}
