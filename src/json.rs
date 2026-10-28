use serde_json::{Map, Number, Value};

const MAX_DEPTH: usize = 512;

pub(super) enum IntegerPolicy {
    RejectOversized,
    WidenToFloat,
}

pub(super) enum Error {
    Syntax,
    DuplicateKey,
    IntegerRange,
    Depth,
}

pub(super) fn parse(bytes: &[u8], integers: IntegerPolicy) -> Result<Value, Error> {
    let mut parser = Parser {
        bytes,
        index: 0,
        depth: 0,
        integers,
    };
    parser.skip_ws();
    if parser.eof() {
        return Err(Error::Syntax);
    }
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.eof() {
        Ok(value)
    } else {
        Err(Error::Syntax)
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
    depth: usize,
    integers: IntegerPolicy,
}

impl Parser<'_> {
    fn parse_value(&mut self) -> Result<Value, Error> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => self.parse_literal(b"true", Value::Bool(true)),
            Some(b'f') => self.parse_literal(b"false", Value::Bool(false)),
            Some(b'n') => self.parse_literal(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(Error::Syntax),
        }
    }

    fn parse_object(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.expect(b'{')?;
        self.skip_ws();
        let mut map = Map::new();
        if self.consume(b'}') {
            self.leave();
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(Error::Syntax);
            }
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value()?;
            if map.insert(key, value).is_some() {
                return Err(Error::DuplicateKey);
            }
            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            if self.consume(b'}') {
                break;
            }
            return Err(Error::Syntax);
        }
        self.leave();
        Ok(Value::Object(map))
    }

    fn parse_array(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.expect(b'[')?;
        self.skip_ws();
        let mut items = Vec::new();
        if self.consume(b']') {
            self.leave();
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            if self.consume(b',') {
                continue;
            }
            if self.consume(b']') {
                break;
            }
            return Err(Error::Syntax);
        }
        self.leave();
        Ok(Value::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut text = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(Error::Syntax);
            };
            match byte {
                b'"' => {
                    self.index += 1;
                    return Ok(text);
                }
                b'\\' => {
                    self.index += 1;
                    self.parse_escape(&mut text)?;
                }
                0x00..=0x1F => return Err(Error::Syntax),
                _ => self.push_char(&mut text)?,
            }
        }
    }

    fn parse_escape(&mut self, text: &mut String) -> Result<(), Error> {
        let Some(byte) = self.next() else {
            return Err(Error::Syntax);
        };
        match byte {
            b'"' | b'\\' | b'/' => text.push(byte as char),
            b'b' => text.push('\u{0008}'),
            b'f' => text.push('\u{000c}'),
            b'n' => text.push('\n'),
            b'r' => text.push('\r'),
            b't' => text.push('\t'),
            b'u' => {
                let unit = self.hex4()?;
                if (0xD800..=0xDBFF).contains(&unit) {
                    self.expect(b'\\')?;
                    self.expect(b'u')?;
                    let low = self.hex4()?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return Err(Error::Syntax);
                    }
                    let code =
                        0x10000 + (((u32::from(unit) - 0xD800) << 10) | (u32::from(low) - 0xDC00));
                    text.push(char::from_u32(code).ok_or(Error::Syntax)?);
                } else if (0xDC00..=0xDFFF).contains(&unit) {
                    return Err(Error::Syntax);
                } else {
                    text.push(char::from_u32(u32::from(unit)).ok_or(Error::Syntax)?);
                }
            }
            _ => return Err(Error::Syntax),
        }
        Ok(())
    }

    fn parse_number(&mut self) -> Result<Value, Error> {
        let start = self.index;
        if self.peek() == Some(b'-') {
            self.index += 1;
        }
        match self.peek() {
            Some(b'0') => self.index += 1,
            Some(b'1'..=b'9') => {
                self.index += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.index += 1;
                }
            }
            _ => return Err(Error::Syntax),
        }
        let mut integer = true;
        if self.consume(b'.') {
            integer = false;
            let fraction = self.index;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
            if self.index == fraction {
                return Err(Error::Syntax);
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            integer = false;
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            let exponent = self.index;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
            if self.index == exponent {
                return Err(Error::Syntax);
            }
        }
        let text =
            std::str::from_utf8(&self.bytes[start..self.index]).map_err(|_| Error::Syntax)?;
        if integer {
            self.finish_integer(text)
        } else {
            finish_float(text)
        }
    }

    fn finish_integer(&self, text: &str) -> Result<Value, Error> {
        let parsed = if text.starts_with('-') {
            text.parse::<i64>().ok().map(Number::from)
        } else {
            text.parse::<u64>().ok().map(Number::from)
        };
        match parsed {
            Some(number) => Ok(Value::Number(number)),
            None => match self.integers {
                IntegerPolicy::RejectOversized => Err(Error::IntegerRange),
                IntegerPolicy::WidenToFloat => finish_float(text),
            },
        }
    }

    fn parse_literal(&mut self, literal: &[u8], value: Value) -> Result<Value, Error> {
        if self.bytes[self.index..].starts_with(literal) {
            self.index += literal.len();
            Ok(value)
        } else {
            Err(Error::Syntax)
        }
    }

    fn push_char(&mut self, text: &mut String) -> Result<(), Error> {
        let rest = &self.bytes[self.index..];
        let width = utf8_width(rest.first().copied().ok_or(Error::Syntax)?).ok_or(Error::Syntax)?;
        if rest.len() < width {
            return Err(Error::Syntax);
        }
        let ch = std::str::from_utf8(&rest[..width])
            .map_err(|_| Error::Syntax)?
            .chars()
            .next()
            .ok_or(Error::Syntax)?;
        text.push(ch);
        self.index += width;
        Ok(())
    }

    fn hex4(&mut self) -> Result<u16, Error> {
        let mut value = 0u16;
        for _ in 0..4 {
            let Some(byte) = self.next() else {
                return Err(Error::Syntax);
            };
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(Error::Syntax),
            };
            value = (value << 4) | u16::from(digit);
        }
        Ok(value)
    }

    fn enter(&mut self) -> Result<(), Error> {
        if self.depth == MAX_DEPTH {
            return Err(Error::Depth);
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.index += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), Error> {
        if self.consume(byte) {
            Ok(())
        } else {
            Err(Error::Syntax)
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.index += 1;
        Some(byte)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn eof(&self) -> bool {
        self.index >= self.bytes.len()
    }
}

fn finish_float(text: &str) -> Result<Value, Error> {
    let number = text.parse::<f64>().map_err(|_| Error::Syntax)?;
    Number::from_f64(number)
        .map(Value::Number)
        .ok_or(Error::Syntax)
}

fn utf8_width(byte: u8) -> Option<usize> {
    if byte < 0x80 {
        Some(1)
    } else if byte & 0xE0 == 0xC0 {
        Some(2)
    } else if byte & 0xF0 == 0xE0 {
        Some(3)
    } else if byte & 0xF8 == 0xF0 {
        Some(4)
    } else {
        None
    }
}
