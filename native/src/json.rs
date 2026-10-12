// M2: compact JSON parser (std only, no serde). Enough for vanilla worldgen JSONs.
// Numbers parse via f64::from_str — correctly rounded, bit-identical to
// Java's Double.parseDouble for the decimal literals vanilla emits.

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().map(|v| v as i32)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn is_obj(&self) -> bool {
        matches!(self, Json::Obj(_))
    }
}

pub fn parse(input: &str) -> Result<Json, String> {
    let bytes = input.as_bytes();
    let mut p = Parser { b: bytes, i: 0, depth: 0 };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.i != bytes.len() {
        return Err(format!("trailing data at {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at {}", c as char, self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.depth += 1;
        if self.depth > 512 {
            return Err("json too deep".into());
        }
        let r = match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(_) => self.number(),
            None => Err("unexpected end".into()),
        };
        self.depth -= 1;
        r
    }

    fn lit(&mut self, s: &str, v: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            Ok(v)
        } else {
            Err(format!("bad literal at {}", self.i))
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(pairs));
        }
        loop {
            self.skip_ws();
            let k = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let v = self.value()?;
            pairs.push((k, v));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(pairs));
                }
                _ => return Err(format!("expected , or }} at {}", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("expected , or ] at {}", self.i)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err("unterminated string".into()),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(s);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or("bad escape")?;
                    self.i += 1;
                    match c {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hex = self
                                .b
                                .get(self.i..self.i + 4)
                                .ok_or("bad \\u")?;
                            let cp = u32::from_str_radix(
                                std::str::from_utf8(hex).map_err(|_| "bad \\u")?,
                                16,
                            )
                            .map_err(|_| "bad \\u")?;
                            self.i += 4;
                            // handle surrogate pairs
                            if (0xD800..0xDC00).contains(&cp) {
                                if self.b.get(self.i) == Some(&b'\\')
                                    && self.b.get(self.i + 1) == Some(&b'u')
                                {
                                    let hex2 = self.b.get(self.i + 2..self.i + 6).ok_or("bad \\u")?;
                                    let lo = u32::from_str_radix(
                                        std::str::from_utf8(hex2).map_err(|_| "bad \\u")?,
                                        16,
                                    )
                                    .map_err(|_| "bad \\u")?;
                                    if (0xDC00..0xE000).contains(&lo) {
                                        self.i += 6;
                                        let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                        s.push(char::from_u32(c).unwrap_or('\u{fffd}'));
                                        continue;
                                    }
                                }
                                s.push('\u{fffd}');
                            } else {
                                s.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err("bad escape".into()),
                    }
                }
                Some(_) => {
                    // consume one UTF-8 code point
                    let start = self.i;
                    self.i += 1;
                    while self.i < self.b.len() && (self.b[self.i] & 0xC0) == 0x80 {
                        self.i += 1;
                    }
                    let chunk = std::str::from_utf8(&self.b[start..self.i])
                        .map_err(|_| "invalid utf8")?;
                    s.push_str(chunk);
                }
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => self.i += 1,
                _ => break,
            }
        }
        let txt = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "bad num")?;
        txt.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number '{}' at {}", txt, start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let v = parse(r#"{"a":1,"b":[true,null,-2.5e2,"x\ny"]}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_f64(), Some(1.0));
        assert_eq!(v.get("b").unwrap().as_arr().unwrap().len(), 4);
        assert_eq!(
            v.get("b").unwrap().as_arr().unwrap()[3],
            Json::Str("x\ny".into())
        );
    }

    #[test]
    fn nested() {
        let v = parse(r#"{"type":"minecraft:min","argument1":{"type":"minecraft:squeeze"}}"#).unwrap();
        assert_eq!(v.get("type").unwrap().as_str(), Some("minecraft:min"));
        assert!(v.get("argument1").unwrap().get("type").is_some());
    }
}
