//! The reader: text to syntax.

use crate::error::Error;
use crate::source::Span;
use crate::symbol::Interner;
use crate::syntax::{Datum, Ident, Syntax};

struct Reader<'a> {
    text: &'a str,
    file: u32,
    pos: usize,
    symbols: &'a mut Interner,
    /// How many brackets are open.
    depth: usize,
}

/// How deep brackets can be nested. Code that deep is a mistake, and what
/// works on the code can then count on an end to it.
const MAX_DEPTH: usize = 1000;

fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '"' | ';' | '\'' | '`' | ',')
}

/// Reads a number written in `radix`, or `None` if the text is not one.
pub fn parse_number(text: &str, radix: u32) -> Option<Datum> {
    let (text, radix) = match text.get(..2) {
        Some("#x" | "#X") => (&text[2..], 16),
        Some("#b" | "#B") => (&text[2..], 2),
        Some("#o" | "#O") => (&text[2..], 8),
        Some("#d" | "#D") => (&text[2..], 10),
        Some("#e" | "#E") => {
            // Exact: a float with no fraction becomes an integer.
            return match parse_number(&text[2..], radix)? {
                Datum::Float(value) if value.fract() == 0.0 && value.abs() < 9.0e18 => {
                    Some(Datum::Int(value as i64))
                }
                other => Some(other),
            };
        }
        Some("#i" | "#I") => {
            return match parse_number(&text[2..], radix)? {
                Datum::Int(value) => Some(Datum::Float(value as f64)),
                other => Some(other),
            };
        }
        _ => (text, radix),
    };
    match text {
        "+inf.0" => return Some(Datum::Float(f64::INFINITY)),
        "-inf.0" => return Some(Datum::Float(f64::NEG_INFINITY)),
        "+nan.0" | "-nan.0" => return Some(Datum::Float(f64::NAN)),
        _ => {}
    }
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() {
        return None;
    }
    if digits.chars().all(|c| c.is_digit(radix)) {
        return match i64::from_str_radix(text, radix) {
            Ok(value) => Some(Datum::Int(value)),
            // Too big to be exact here: the nearest float.
            Err(_) if radix == 10 => text.parse().ok().map(Datum::Float),
            Err(_) => None,
        };
    }
    if radix != 10 {
        return None;
    }
    // A ratio is read as the float it comes to.
    if let Some((numerator, denominator)) = text.split_once('/') {
        let is_integer = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
        let unsigned = numerator.strip_prefix(['+', '-']).unwrap_or(numerator);
        if is_integer(unsigned) && is_integer(denominator) {
            let (top, bottom) = (
                numerator.parse::<f64>().ok()?,
                denominator.parse::<f64>().ok()?,
            );
            let value = top / bottom;
            return Some(if value.fract() == 0.0 && bottom != 0.0 {
                Datum::Int(value as i64)
            } else {
                Datum::Float(value)
            });
        }
        return None;
    }
    // What Rust takes for a float is more than Scheme does.
    let shaped = digits.starts_with(|c: char| c.is_ascii_digit() || c == '.')
        && digits.chars().any(|c| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    if shaped {
        text.parse().ok().map(Datum::Float)
    } else {
        None
    }
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn span(&self, start: usize) -> Span {
        Span {
            file: self.file,
            start: start as u32,
            end: self.pos as u32,
        }
    }

    fn error(&self, start: usize, message: &str) -> Error {
        Error::new(message).at(self.span(start))
    }

    /// Skips whitespace and comments. Fails on a comment that never ends.
    fn skip_blanks(&mut self) -> Result<(), Error> {
        loop {
            let start = self.pos;
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some(';') => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
                Some('#') if self.text[self.pos..].starts_with("#|") => {
                    self.pos += 2;
                    let mut depth = 1;
                    while depth > 0 {
                        if self.text[self.pos..].starts_with("|#") {
                            self.pos += 2;
                            depth -= 1;
                        } else if self.text[self.pos..].starts_with("#|") {
                            self.pos += 2;
                            depth += 1;
                        } else if self.bump().is_none() {
                            return Err(self.error(start, "this comment is never closed"));
                        }
                    }
                }
                Some('#') if self.text[self.pos..].starts_with("#;") => {
                    self.pos += 2;
                    // The datum after it is read and dropped.
                    if self.datum()?.is_none() {
                        return Err(self.error(start, "nothing follows #; to comment out"));
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// Reads up to the next delimiter.
    fn token(&mut self) -> &str {
        let start = self.pos;
        while self.peek().is_some_and(|c| !is_delimiter(c)) {
            self.bump();
        }
        &self.text[start..self.pos]
    }

    fn list(&mut self, start: usize, close: char) -> Result<Syntax, Error> {
        let mut items = Vec::new();
        loop {
            self.skip_blanks()?;
            let here = self.pos;
            match self.peek() {
                None => return Err(self.error(start, "this bracket is never closed")),
                Some(c @ (')' | ']')) => {
                    self.bump();
                    if c != close {
                        return Err(self.error(
                            here,
                            &format!("expected `{close}` to close the bracket, found `{c}`"),
                        ));
                    }
                    return Ok(Syntax::new(Datum::List(items), self.span(start)));
                }
                // A dot on its own, before the last element.
                Some('.')
                    if self.text[self.pos + 1..]
                        .chars()
                        .next()
                        .is_none_or(is_delimiter) =>
                {
                    self.bump();
                    let Some(tail) = self.datum()? else {
                        return Err(self.error(here, "nothing follows the dot"));
                    };
                    self.skip_blanks()?;
                    if self.bump() != Some(close) {
                        return Err(self.error(here, "one datum must follow the dot"));
                    }
                    if items.is_empty() {
                        return Err(self.error(here, "nothing comes before the dot"));
                    }
                    // `(a . (b c))` is the list `(a b c)`.
                    return Ok(match tail.datum {
                        Datum::List(rest) => {
                            items.extend(rest);
                            Syntax::new(Datum::List(items), self.span(start))
                        }
                        Datum::Dotted(rest, last) => {
                            items.extend(rest);
                            Syntax::new(Datum::Dotted(items, last), self.span(start))
                        }
                        _ => Syntax::new(Datum::Dotted(items, Box::new(tail)), self.span(start)),
                    });
                }
                Some(_) => match self.datum()? {
                    Some(item) => items.push(item),
                    None => return Err(self.error(start, "this bracket is never closed")),
                },
            }
        }
    }

    fn string(&mut self, start: usize) -> Result<Syntax, Error> {
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.error(start, "this string is never closed")),
                Some('"') => break,
                Some('\\') => {
                    let escape = self.pos - 1;
                    match self.bump() {
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        Some('r') => out.push('\r'),
                        Some('a') => out.push('\u{7}'),
                        Some('b') => out.push('\u{8}'),
                        Some('0') => out.push('\0'),
                        Some('e') => out.push('\u{1b}'),
                        Some(c @ ('"' | '\\' | '|')) => out.push(c),
                        Some('x') => {
                            let digits_start = self.pos;
                            while self.peek().is_some_and(|c| c != ';' && c != '"') {
                                self.bump();
                            }
                            let digits = &self.text[digits_start..self.pos];
                            let c = u32::from_str_radix(digits, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .filter(|_| self.peek() == Some(';'));
                            let Some(c) = c else {
                                return Err(
                                    self.error(escape, "a char escape is written \\x41; in hex")
                                );
                            };
                            self.bump();
                            out.push(c);
                        }
                        // A backslash at the end of a line continues the
                        // string on the next, without the indentation.
                        Some(c) if c == '\n' || c == ' ' || c == '\t' => {
                            let rest = &self.text[self.pos - 1..];
                            let line_end = rest.find('\n');
                            let only_blanks = line_end.is_some_and(|end| {
                                rest[..end].chars().all(|c| c == ' ' || c == '\t')
                            });
                            if !only_blanks {
                                return Err(self.error(escape, "unknown escape in a string"));
                            }
                            self.pos += line_end.unwrap_or(0);
                            while self.peek().is_some_and(|c| c == ' ' || c == '\t') {
                                self.bump();
                            }
                        }
                        _ => return Err(self.error(escape, "unknown escape in a string")),
                    }
                }
                Some(c) => out.push(c),
            }
        }
        Ok(Syntax::new(Datum::Str(out.into()), self.span(start)))
    }

    fn hash(&mut self, start: usize) -> Result<Syntax, Error> {
        // Past the `#`.
        match self.peek() {
            // There are no bytevectors: `#u8(...)` is a vector of numbers.
            Some('u') if self.text[self.pos..].starts_with("u8(") => {
                self.pos += 2;
                self.hash(start)
            }
            Some('(') => {
                self.bump();
                let list = self.list(start, ')')?;
                let Datum::List(items) = list.datum else {
                    return Err(self.error(start, "a vector cannot have a dot in it"));
                };
                Ok(Syntax::new(Datum::Vector(items), self.span(start)))
            }
            Some('\\') => {
                self.bump();
                // The char itself may be a delimiter, as in `#\(`.
                let first = self
                    .bump()
                    .ok_or_else(|| self.error(start, "no char follows #\\"))?;
                let rest = self.token();
                let name = format!("{first}{rest}");
                let c = match name.as_str() {
                    _ if rest.is_empty() => first,
                    "space" => ' ',
                    "newline" | "linefeed" | "nl" => '\n',
                    "tab" => '\t',
                    "return" => '\r',
                    "null" | "nul" => '\0',
                    "alarm" => '\u{7}',
                    "backspace" => '\u{8}',
                    "delete" | "rubout" => '\u{7f}',
                    "escape" | "altmode" => '\u{1b}',
                    hex if first == 'x' || first == 'U' || first == 'u' => {
                        u32::from_str_radix(&hex[1..], 16)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| self.error(start, "no such char"))?
                    }
                    _ => return Err(self.error(start, "no such char")),
                };
                Ok(Syntax::new(Datum::Char(c), self.span(start)))
            }
            Some(':') => {
                self.bump();
                let name = self.token().to_owned();
                if name.is_empty() {
                    return Err(self.error(start, "a keyword needs a name after #:"));
                }
                let symbol = self.symbols.intern(&name);
                Ok(Syntax::new(Datum::Keyword(symbol), self.span(start)))
            }
            _ => {
                self.pos = start;
                let token = self.token().to_owned();
                let datum = match token.as_str() {
                    "#t" | "#true" => Datum::Bool(true),
                    "#f" | "#false" => Datum::Bool(false),
                    other => parse_number(other, 10)
                        .ok_or_else(|| self.error(start, &format!("unknown syntax: {other}")))?,
                };
                Ok(Syntax::new(datum, self.span(start)))
            }
        }
    }

    /// Reads one datum, or `None` at the end of the text.
    fn datum(&mut self) -> Result<Option<Syntax>, Error> {
        if self.depth > MAX_DEPTH {
            return Err(self.error(self.pos, "brackets are nested too deeply"));
        }
        self.depth += 1;
        let datum = crate::deep(|| self.datum_here());
        self.depth -= 1;
        datum
    }

    fn datum_here(&mut self) -> Result<Option<Syntax>, Error> {
        self.skip_blanks()?;
        let start = self.pos;
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let quoted = |reader: &mut Self, name: &str, len: usize| -> Result<Syntax, Error> {
            reader.pos += len;
            let head = reader.symbols.intern(name);
            let Some(inner) = reader.datum()? else {
                return Err(reader.error(start, "nothing follows the quote"));
            };
            let head = Syntax::new(Datum::Ident(Ident::plain(head)), reader.span(start));
            Ok(Syntax::new(
                Datum::List(vec![head, inner]),
                reader.span(start),
            ))
        };
        let syntax = match c {
            '(' => {
                self.bump();
                self.list(start, ')')?
            }
            '[' => {
                self.bump();
                self.list(start, ']')?
            }
            ')' | ']' => {
                self.bump();
                return Err(self.error(start, "this closes a bracket that was never opened"));
            }
            '"' => {
                self.bump();
                self.string(start)?
            }
            '\'' => quoted(self, "quote", 1)?,
            '`' => quoted(self, "quasiquote", 1)?,
            ',' if self.text[self.pos..].starts_with(",@") => quoted(self, "unquote-splicing", 2)?,
            ',' => quoted(self, "unquote", 1)?,
            '#' => {
                self.bump();
                self.hash(start)?
            }
            '|' => {
                // A symbol with any chars in it, between bars.
                self.bump();
                let name_start = self.pos;
                while self.peek().is_some_and(|c| c != '|') {
                    self.bump();
                }
                let name = self.text[name_start..self.pos].to_owned();
                if self.bump().is_none() {
                    return Err(self.error(start, "this symbol is never closed"));
                }
                let symbol = self.symbols.intern(&name);
                Syntax::new(Datum::Ident(Ident::plain(symbol)), self.span(start))
            }
            _ => {
                let token = self.token().to_owned();
                let datum = match parse_number(&token, 10) {
                    Some(number) => number,
                    None => Datum::Ident(Ident::plain(self.symbols.intern(&token))),
                };
                Syntax::new(datum, self.span(start))
            }
        };
        Ok(Some(syntax))
    }
}

/// Reads the next datum in a text from a position, and gives it with the
/// position after it; `None` at the end. `file` is the text's number among
/// the sources, which the positions will refer to.
pub fn read_next(
    text: &str,
    file: u32,
    pos: usize,
    symbols: &mut Interner,
) -> Result<Option<(Syntax, usize)>, Error> {
    let mut reader = Reader {
        text,
        file,
        pos,
        symbols,
        depth: 0,
    };
    Ok(reader.datum()?.map(|datum| (datum, reader.pos)))
}

/// Reads every datum in a text.
#[cfg(test)]
pub fn read_all(text: &str, file: u32, symbols: &mut Interner) -> Result<Vec<Syntax>, Error> {
    let mut forms = Vec::new();
    let mut pos = 0;
    while let Some((form, after)) = read_next(text, file, pos, symbols)? {
        forms.push(form);
        pos = after;
    }
    Ok(forms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The datum written back out, to compare what was understood.
    fn show(syntax: &Syntax, symbols: &Interner) -> String {
        match &syntax.datum {
            Datum::Bool(true) => "#t".to_owned(),
            Datum::Bool(false) => "#f".to_owned(),
            Datum::Int(value) => value.to_string(),
            Datum::Float(value) => format!("{value:?}"),
            Datum::Char(c) => format!("#\\{c}"),
            Datum::Str(text) => format!("{text:?}"),
            Datum::Ident(ident) => symbols.name(ident.name).to_owned(),
            Datum::Keyword(name) => format!("#:{}", symbols.name(*name)),
            Datum::List(items) => {
                let items: Vec<String> = items.iter().map(|item| show(item, symbols)).collect();
                format!("({})", items.join(" "))
            }
            Datum::Dotted(items, tail) => {
                let items: Vec<String> = items.iter().map(|item| show(item, symbols)).collect();
                format!("({} . {})", items.join(" "), show(tail, symbols))
            }
            Datum::Vector(items) => {
                let items: Vec<String> = items.iter().map(|item| show(item, symbols)).collect();
                format!("#({})", items.join(" "))
            }
        }
    }

    fn read(text: &str) -> String {
        let mut symbols = Interner::default();
        match read_all(text, 0, &mut symbols) {
            Ok(forms) => forms
                .iter()
                .map(|form| show(form, &symbols))
                .collect::<Vec<_>>()
                .join(" "),
            Err(error) => format!("error at {}: {}", error.span.unwrap().start, error.message),
        }
    }

    #[test]
    fn data_are_read() {
        assert_eq!(read("(a [b 1] . c)"), "(a (b 1) . c)");
        assert_eq!(read("(a . (b c))"), "(a b c)");
        assert_eq!(
            read("'a `(b ,c ,@d)"),
            "(quote a) (quasiquote (b (unquote c) (unquote-splicing d)))"
        );
        assert_eq!(read("#(1 #t #false) #:key"), "#(1 #t #f) #:key");
        assert_eq!(read(r#""a\n\"b\"\x41;""#), r#""a\n\"b\"A""#);
        assert_eq!(read("\"one \\\n   two\""), "\"one two\"");
        assert_eq!(
            read(r"#\a #\space #\( #\x41 #\newline"),
            "#\\a #\\  #\\( #\\A #\\\n"
        );
        assert_eq!(read("|hello world| ... -> a.b"), "hello world ... -> a.b");
    }

    #[test]
    fn numbers_are_told_from_symbols() {
        assert_eq!(
            read("1 -2 +3 1.5 -.5 1e3 #x1F #b101 1/2 4/2"),
            "1 -2 3 1.5 -0.5 1000.0 31 5 0.5 2"
        );
        assert_eq!(
            read("+ - ... 1+ -a +inf.0 1.2.3 e"),
            "+ - ... 1+ -a inf 1.2.3 e"
        );
        assert_eq!(read("#e1.0 #i2 99999999999999999999"), "1 2.0 1e20");
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(read("a ; b\n c #| d #| e |# f |# g #;(h i) j"), "a c g j");
    }

    #[test]
    fn mistakes_are_reported_where_they_are() {
        assert_eq!(read("(a (b)"), "error at 0: this bracket is never closed");
        assert_eq!(
            read("(a))"),
            "error at 3: this closes a bracket that was never opened"
        );
        assert_eq!(
            read("(a ]"),
            "error at 3: expected `)` to close the bracket, found `]`"
        );
        assert_eq!(read("\"abc"), "error at 0: this string is never closed");
        assert_eq!(read("(. a)"), "error at 1: nothing comes before the dot");
        assert_eq!(read("#|"), "error at 0: this comment is never closed");
        assert_eq!(read("#\\nosuch"), "error at 0: no such char");
        assert_eq!(read("#z"), "error at 0: unknown syntax: #z");
    }

    #[test]
    fn positions_cover_each_datum() {
        let mut symbols = Interner::default();
        let forms = read_all("  (ab  cd)", 3, &mut symbols).unwrap();
        assert_eq!((forms[0].span.start, forms[0].span.end), (2, 10));
        let inner = forms[0].list().unwrap();
        assert_eq!((inner[1].span.start, inner[1].span.end), (7, 9));
        assert_eq!(inner[1].span.file, 3);
    }
}
