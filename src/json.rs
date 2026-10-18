//! 够用的 JSON 解析：只读不写。读的都是 Claude Code 自己生成的文件和接口返回，格式规整；
//! 不引 serde_json，exe 小一圈，每行只解析一次，速度足够（见 usage.rs 的扫描耗时）。

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(HashMap<String, Json>),
}

static NULL: Json = Json::Null;

impl Json {
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = P { b: s.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        (p.i == p.b.len()).then_some(v)
    }

    /// 取字段；不是对象或没有这个字段时返回 Null，可以连着写 j.get("a").get("b")
    pub fn get(&self, k: &str) -> &Json {
        match self {
            Json::Obj(m) => m.get(k).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn str(&self) -> Option<&str> {
        if let Json::Str(s) = self { Some(s) } else { None }
    }

    pub fn num(&self) -> Option<f64> {
        if let Json::Num(n) = self { Some(*n) } else { None }
    }

    pub fn u64(&self) -> u64 {
        self.num().map_or(0, |n| n as u64)
    }

    pub fn arr(&self) -> &[Json] {
        if let Json::Arr(a) = self { a } else { &[] }
    }
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> Option<()> {
        self.b[self.i..].starts_with(lit.as_bytes()).then(|| self.i += lit.len())
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = HashMap::new();
                self.ws();
                if self.eat("}").is_some() {
                    return Some(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.eat(":")?;
                    let v = self.value()?;
                    m.insert(k, v);
                    self.ws();
                    if self.eat(",").is_none() {
                        self.eat("}")?;
                        return Some(Json::Obj(m));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat("]").is_some() {
                    return Some(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    if self.eat(",").is_none() {
                        self.eat("]")?;
                        return Some(Json::Arr(a));
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.eat("true").map(|_| Json::Bool(true)),
            b'f' => self.eat("false").map(|_| Json::Bool(false)),
            b'n' => self.eat("null").map(|_| Json::Null),
            _ => {
                let st = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[st..self.i]).ok()?.parse().ok().map(Json::Num)
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        self.eat("\"")?;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let mut cp = self.hex4()?;
                            // 代理对
                            if (0xD800..0xDC00).contains(&cp) && self.eat("\\u").is_some() {
                                let lo = self.hex4()?;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            char::from_u32(cp).unwrap_or('\u{FFFD}')
                        }
                        c => c as char,
                    };
                    let mut buf = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let s = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(s, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let j = Json::parse(r#" {"a":[1,-2.5e1,true,null],"s":"x\"\\\u00e9\ud83d\ude00\n","o":{}} "#).unwrap();
        assert_eq!(j.get("a").arr()[1].num(), Some(-25.0));
        assert_eq!(j.get("a").arr()[2], Json::Bool(true));
        assert_eq!(j.get("s").str(), Some("x\"\\é😀\n"));
        assert_eq!(j.get("o").get("missing"), &Json::Null);
        assert_eq!(Json::parse(r#"{"中文":"值"}"#).unwrap().get("中文").str(), Some("值"));
        assert!(Json::parse("{\"a\":1").is_none());
        assert!(Json::parse("[1,2] x").is_none());
    }
}
