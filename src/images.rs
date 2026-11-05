//! Screenshot attachments carry PNG bytes in each saved user message.
use serde_json::{Value, json};
#[derive(Clone)]
pub struct Image {
    pub data: String,
}
impl Image {
    pub fn png(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 5 * 1024 * 1024 {
            return Err("Screenshot exceeds 5 MB".into());
        }
        if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
            return Err("Clipboard does not contain a PNG screenshot".into());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap()) as u64;
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap()) as u64;
        if width == 0 || height == 0 || width * height > 32_000_000 {
            return Err("Screenshot exceeds 32 million pixels".into());
        }
        Ok(Self { data: encode(bytes) })
    }
    pub fn to_json(&self) -> Value {
        json!({"mime":"image/png","data":self.data})
    }
    pub fn from_json(v: &Value) -> Option<Self> {
        let data = v["data"].as_str()?;
        if v["mime"] != "image/png"
            || data.len() > 7 * 1024 * 1024
            || !data.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        {
            return None;
        }
        Some(Self { data: data.into() })
    }
}
pub(crate) fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let value =
            (c[0] as u32) << 16 | (c.get(1).copied().unwrap_or(0) as u32) << 8 | c.get(2).copied().unwrap_or(0) as u32;
        for shift in [18, 12, 6, 0] {
            out.push(ALPHABET[(value >> shift & 63) as usize] as char);
        }
        if c.len() < 3 {
            out.pop();
            out.push('=');
        }
        if c.len() < 2 {
            let n = out.len();
            out.replace_range(n - 2..n - 1, "=");
        }
    }
    out
}
pub fn content(text: String, images: &[Image], protocol: crate::agent::Protocol) -> Value {
    use crate::agent::Protocol;
    if images.is_empty() {
        return text.into();
    }
    let mut blocks = vec![match protocol {
        Protocol::Responses => json!({"type":"input_text","text":text}),
        _ => json!({"type":"text","text":text}),
    }];
    blocks.extend(images.iter().map(|im| match protocol {
        Protocol::Chat => json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}",im.data)}}),
        Protocol::Messages => {
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":im.data}})
        }
        Protocol::Responses => json!({"type":"input_image","image_url":format!("data:image/png;base64,{}",im.data)}),
    }));
    blocks.into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_padding_and_wire_formats() {
        for (input, expected) in [("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")] {
            assert_eq!(encode(input.as_bytes()), expected);
        }
        let im = Image { data: "YWJj".into() };
        assert_eq!(
            content("look".into(), std::slice::from_ref(&im), crate::agent::Protocol::Messages)[1]["source"]["data"],
            "YWJj"
        );
        assert_eq!(Image::from_json(&im.to_json()).unwrap().data, "YWJj");
        assert!(Image::png(b"not png").is_err());
    }
}
