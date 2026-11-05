//! A JSON reader that keeps where each key and value lies, so a schema
//! error can be marked on the text that causes it. It runs after serde_json
//! has accepted the text, so it can trust the syntax.

/// A value and the byte range it spans.
#[derive(Debug, Clone)]
pub struct Node {
    pub span: (usize, usize),
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub enum Kind {
    /// Each member with its key's span, in the order written.
    Object(Vec<(String, (usize, usize), Node)>),
    Array(Vec<Node>),
    Str(String),
    /// The number and whether it was written without a fraction or exponent.
    Num(f64, bool),
    Bool(bool),
    Null,
}

impl Node {
    /// The JSON type name the schemas use.
    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            Kind::Object(_) => "object",
            Kind::Array(_) => "array",
            Kind::Str(_) => "string",
            Kind::Num(_, true) => "integer",
            Kind::Num(..) => "number",
            Kind::Bool(_) => "boolean",
            Kind::Null => "null",
        }
    }

    pub fn value(&self) -> serde_json::Value {
        use serde_json::Value;
        match &self.kind {
            Kind::Object(m) => Value::Object(m.iter().map(|(k, _, v)| (k.clone(), v.value())).collect()),
            Kind::Array(a) => Value::Array(a.iter().map(Node::value).collect()),
            Kind::Str(s) => Value::String(s.clone()),
            Kind::Num(n, true) if n.fract() == 0.0 && n.abs() < 9e15 => Value::from(*n as i64),
            Kind::Num(n, _) => serde_json::json!(n),
            Kind::Bool(b) => Value::Bool(*b),
            Kind::Null => Value::Null,
        }
    }
}

/// Reads `text`, which must be valid JSON; None if it is not.
pub fn parse(text: &str) -> Option<Node> {
    let mut r = Reader { b: text.as_bytes(), text, at: 0, depth: 0 };
    let node = r.value()?;
    r.space();
    (r.at == r.b.len()).then_some(node)
}

struct Reader<'a> {
    b: &'a [u8],
    text: &'a str,
    at: usize,
    depth: usize,
}

impl Reader<'_> {
    fn space(&mut self) {
        while self.at < self.b.len() && matches!(self.b[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Option<Node> {
        self.space();
        let start = self.at;
        self.depth += 1;
        if self.depth > 128 {
            return None;
        }
        let kind = match *self.b.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut members = Vec::new();
                loop {
                    self.space();
                    if self.b.get(self.at) == Some(&b'}') {
                        self.at += 1;
                        break;
                    }
                    let key_at = self.at;
                    let key = self.string()?;
                    let key_span = (key_at, self.at);
                    self.space();
                    (self.b.get(self.at) == Some(&b':')).then_some(())?;
                    self.at += 1;
                    let v = self.value()?;
                    members.push((key, key_span, v));
                    self.space();
                    match self.b.get(self.at)? {
                        b',' => self.at += 1,
                        b'}' => {
                            self.at += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Kind::Object(members)
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.space();
                    if self.b.get(self.at) == Some(&b']') {
                        self.at += 1;
                        break;
                    }
                    items.push(self.value()?);
                    self.space();
                    match self.b.get(self.at)? {
                        b',' => self.at += 1,
                        b']' => {
                            self.at += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Kind::Array(items)
            }
            b'"' => Kind::Str(self.string()?),
            b't' if self.text[self.at..].starts_with("true") => {
                self.at += 4;
                Kind::Bool(true)
            }
            b'f' if self.text[self.at..].starts_with("false") => {
                self.at += 5;
                Kind::Bool(false)
            }
            b'n' if self.text[self.at..].starts_with("null") => {
                self.at += 4;
                Kind::Null
            }
            _ => {
                let end = self.text[self.at..]
                    .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
                    .map_or(self.b.len(), |n| self.at + n);
                let raw = &self.text[self.at..end];
                let n: f64 = raw.parse().ok()?;
                self.at = end;
                Kind::Num(n, !raw.contains(['.', 'e', 'E']))
            }
        };
        self.depth -= 1;
        Some(Node { span: (start, self.at), kind })
    }

    fn string(&mut self) -> Option<String> {
        (self.b.get(self.at) == Some(&b'"')).then_some(())?;
        let start = self.at;
        self.at += 1;
        while self.at < self.b.len() {
            match self.b[self.at] {
                b'\\' => self.at += 2,
                b'"' => {
                    self.at += 1;
                    return serde_json::from_str(&self.text[start..self.at]).ok();
                }
                _ => self.at += 1,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_point_at_keys_and_values() {
        let text = r#"{"a": [1, 2.5, "x\"y"], "b": {"c": true}}"#;
        let n = parse(text).unwrap();
        let Kind::Object(m) = &n.kind else { panic!() };
        assert_eq!(&text[m[0].1.0..m[0].1.1], "\"a\"");
        assert_eq!(&text[m[0].2.span.0..m[0].2.span.1], r#"[1, 2.5, "x\"y"]"#);
        let Kind::Array(items) = &m[0].2.kind else { panic!() };
        assert_eq!(items[0].type_name(), "integer");
        assert_eq!(items[1].type_name(), "number");
        assert!(matches!(&items[2].kind, Kind::Str(s) if s == "x\"y"));
        assert_eq!(n.value(), serde_json::from_str::<serde_json::Value>(text).unwrap());
        assert!(parse("{\"a\": }").is_none());
    }
}
