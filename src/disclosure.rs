use std::fmt::{self, Write};

use crate::{
    runtime::value::{Value, ValueKind},
    types::Type,
};

const SHOW_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct DisclosureLimits {
    pub depth: usize,
    pub max_items: usize,
    pub max_string: usize,
    pub max_output: usize,
}

impl Default for DisclosureLimits {
    fn default() -> Self {
        Self {
            depth: 64,
            max_items: 100_000,
            max_string: 16 * 1024 * 1024,
            max_output: SHOW_OUTPUT,
        }
    }
}

#[derive(Clone, Copy)]
struct Space {
    bytes: usize,
    chars: usize,
}

impl Space {
    fn fits(self, text: &str) -> bool {
        text.len() <= self.bytes
            && (self.chars >= text.len()
                || text.chars().take(self.chars.saturating_add(1)).count() <= self.chars)
    }

    fn after(self, text: &str) -> Self {
        Self {
            bytes: self.bytes.saturating_sub(text.len()),
            chars: if self.chars == usize::MAX {
                usize::MAX
            } else {
                self.chars.saturating_sub(text.chars().count())
            },
        }
    }
}

fn count_label(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}

pub fn size(value: &Value) -> String {
    match value.kind() {
        ValueKind::Array(items) => count_label(items.len(), "item"),
        ValueKind::Object(fields) => count_label(fields.len(), "key"),
        ValueKind::String(text) => count_label(text.chars().count(), "char"),
        ValueKind::Server(_) => match value.display_type() {
            Type::Server(tools) => count_label(tools.len(), "tool"),
            _ => "server".into(),
        },
        ValueKind::Closure(_) | ValueKind::Builtin(_) | ValueKind::Tool { .. } => "function".into(),
        _ => "1 value".into(),
    }
}

pub fn binding_line(name: &str, ty: &Type, value: &Value) -> String {
    let space = Space {
        bytes: 2400,
        chars: 600,
    };
    let name = shortened(
        name,
        Space {
            bytes: 512,
            chars: 128,
        },
    );
    let label = format!(" ({}) = ", size(value));
    let prefix = format!("{name}: ");
    let available = space.after(&prefix).after(&label);
    let ty = type_text(
        ty,
        Space {
            bytes: available.bytes.min(800),
            chars: available.chars.min(200),
        },
    );
    let prefix = format!("{prefix}{ty}{label}");
    let options = Render {
        depth: 2,
        arrays: 5,
        objects: 8,
        strings: 120,
    };
    let preview = options
        .value(value, 0, space.after(&prefix))
        .unwrap_or_else(|| "…".into());
    format!("{prefix}{preview}")
}

pub fn show(value: &Value, limits: DisclosureLimits) -> String {
    let space = Space {
        bytes: limits.max_output.min(SHOW_OUTPUT),
        chars: usize::MAX,
    };
    let options = Render {
        depth: limits.depth.min(64),
        arrays: limits.max_items.min(100_000),
        objects: limits.max_items.min(100_000),
        strings: limits.max_string.min(16 * 1024 * 1024),
    };
    if let ValueKind::String(text) = value.kind() {
        if let Some((quoted, false)) = quote(text, options.strings, space) {
            return quoted;
        }
        let prefix = format!("string ({}) = ", size(value));
        if space.fits(&prefix)
            && let Some((quoted, _)) = quote(text, options.strings, space.after(&prefix))
        {
            return format!("{prefix}{quoted}");
        }
    } else if let Some(rendered) = options.value(value, 0, space) {
        return rendered;
    }
    let suffix = format!(" ({})", size(value));
    if space.fits(&suffix) {
        format!(
            "{}{suffix}",
            type_text(value.display_type(), space.after(&suffix))
        )
    } else {
        shortened(&suffix, space)
    }
}

struct Render {
    depth: usize,
    arrays: usize,
    objects: usize,
    strings: usize,
}

impl Render {
    fn value(&self, value: &Value, depth: usize, space: Space) -> Option<String> {
        let text = match value.kind() {
            ValueKind::Null => "null".into(),
            ValueKind::Bool(value) => value.to_string(),
            ValueKind::Number(value) => {
                if *value == 0.0 {
                    "0".into()
                } else {
                    value.to_string()
                }
            }
            ValueKind::String(text) => {
                return quote(text, self.strings, space).map(|(text, _)| text);
            }
            ValueKind::Array(items) => {
                return self.container(items.len(), false, depth, space, |index, child_space| {
                    self.value(&items[index], depth + 1, child_space)
                });
            }
            ValueKind::Object(fields) => {
                return self.container(fields.len(), true, depth, space, |index, child_space| {
                    let (key, value) = &fields[index];
                    let key = key_text(key, child_space)?;
                    let prefix = format!("{key}: ");
                    if !child_space.fits(&prefix) {
                        return None;
                    }
                    let value = self.value(value, depth + 1, child_space.after(&prefix))?;
                    Some(format!("{prefix}{value}"))
                });
            }
            _ => type_text(value.display_type(), space),
        };
        (!text.is_empty() && space.fits(&text)).then_some(text)
    }

    fn container(
        &self,
        len: usize,
        object: bool,
        depth: usize,
        space: Space,
        child: impl Fn(usize, Space) -> Option<String>,
    ) -> Option<String> {
        let (open, close) = if object { ("{", "}") } else { ("[", "]") };
        if len == 0 {
            let text = format!("{open}{close}");
            return space.fits(&text).then_some(text);
        }
        if depth >= self.depth {
            let text = format!("{open}…{close}");
            return space.fits(&text).then_some(text);
        }
        let marker = |remaining| {
            if object {
                format!("…+{remaining} keys")
            } else {
                format!("…+{remaining}")
            }
        };
        let minimal = format!("{open}{}{close}", marker(len));
        if !space.fits(&minimal) {
            return None;
        }
        let mut out = open.to_owned();
        let mut shown = 0;
        let max = len.min(if object { self.objects } else { self.arrays });
        while shown < max {
            let separator = if shown == 0 { "" } else { ", " };
            let suffix = if shown + 1 < len {
                format!(", {}{close}", marker(len - shown - 1))
            } else {
                close.into()
            };
            let remaining = space.after(&out);
            if !remaining.fits(&format!("{separator}{suffix}")) {
                break;
            }
            let Some(text) = child(shown, remaining.after(separator).after(&suffix)) else {
                break;
            };
            out.push_str(separator);
            out.push_str(&text);
            shown += 1;
        }
        if shown < len {
            if shown > 0 {
                out.push_str(", ");
            }
            out.push_str(&marker(len - shown));
        }
        out.push_str(close);
        Some(out)
    }
}

fn escaped(ch: char) -> String {
    match ch {
        '"' => "\\\"".into(),
        '\\' => "\\\\".into(),
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        '\u{08}' => "\\b".into(),
        '\u{0c}' => "\\f".into(),
        '\u{00}'..='\u{1f}' => format!("\\u{:04x}", ch as u32),
        _ => ch.to_string(),
    }
}

fn quote(text: &str, max_chars: usize, space: Space) -> Option<(String, bool)> {
    if !space.fits("\"\"") {
        return None;
    }
    let mut out = String::from("\"");
    let mut remaining = space.after("\"\"");
    let mut boundaries = Vec::new();
    let mut chars = text.chars().peekable();
    for _ in 0..max_chars {
        let Some(&ch) = chars.peek() else {
            break;
        };
        let chunk = escaped(ch);
        if !remaining.fits(&chunk) {
            break;
        }
        boundaries.push((out.len(), remaining));
        out.push_str(&chunk);
        remaining = remaining.after(&chunk);
        chars.next();
    }
    let truncated = chars.peek().is_some();
    if truncated {
        while !remaining.fits("…") {
            let (length, restored) = boundaries.pop()?;
            out.truncate(length);
            remaining = restored;
        }
        out.push('…');
    }
    out.push('"');
    Some((out, truncated))
}

fn bare_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 128 {
        return false;
    }
    let mut parts = key.split('.');
    let root = parts.next().unwrap_or_default();
    let Some(first) = root.bytes().next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && !matches!(root, "true" | "false" | "null")
        && std::iter::once(root).chain(parts).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
}

fn key_text(key: &str, space: Space) -> Option<String> {
    if bare_key(key) {
        return space.fits(key).then(|| key.to_owned());
    }
    let (text, truncated) = quote(key, usize::MAX, space)?;
    (!truncated).then_some(text)
}

fn shortened(text: &str, space: Space) -> String {
    let mut writer = TypeWriter::new(space);
    let _ = writer.write_str(text);
    writer.finish()
}

fn type_text(ty: &Type, space: Space) -> String {
    let mut writer = TypeWriter::new(space);
    let _ = writer.ty(ty, 0, 0);
    writer.finish()
}

// Stop at the writer boundary, including inside names and literal strings.
struct TypeWriter {
    out: String,
    space: Space,
    truncated: bool,
}

impl TypeWriter {
    fn new(space: Space) -> Self {
        Self {
            out: String::new(),
            space,
            truncated: false,
        }
    }

    fn finish(mut self) -> String {
        if self.truncated {
            while !self.space.after(&self.out).fits("…") && !self.out.is_empty() {
                self.out.pop();
            }
            if self.space.after(&self.out).fits("…") {
                self.out.push('…');
            }
        }
        self.out
    }

    fn key(&mut self, key: &str, force_quotes: bool) -> fmt::Result {
        if !force_quotes && bare_key(key) {
            return self.write_str(key);
        }
        self.write_char('"')?;
        for ch in key.chars() {
            self.write_str(&escaped(ch))?;
        }
        self.write_char('"')
    }

    fn signature(&mut self, signature: &crate::types::FunctionType, depth: usize) -> fmt::Result {
        self.write_char('(')?;
        for (index, parameter) in signature.parameters.iter().enumerate() {
            if index > 0 {
                self.write_str(", ")?;
            }
            self.key(&parameter.name, false)?;
            self.write_str(": ")?;
            self.ty(&parameter.ty, 0, depth + 1)?;
        }
        self.write_str(") => ")?;
        self.ty(&signature.result, 0, depth + 1)
    }

    fn literal(&mut self, value: &serde_json::Value, depth: usize) -> fmt::Result {
        if depth > 64 {
            self.truncated = true;
            return Err(fmt::Error);
        }
        match value {
            serde_json::Value::String(text) => self.key(text, true),
            serde_json::Value::Array(items) => {
                self.write_char('[')?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.write_char(',')?;
                    }
                    self.literal(item, depth + 1)?;
                }
                self.write_char(']')
            }
            serde_json::Value::Object(fields) => {
                self.write_char('{')?;
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        self.write_char(',')?;
                    }
                    self.key(key, true)?;
                    self.write_char(':')?;
                    self.literal(value, depth + 1)?;
                }
                self.write_char('}')
            }
            serde_json::Value::Number(number) => match number.as_f64() {
                Some(number) => write!(self, "{}", if number == 0.0 { 0.0 } else { number }),
                None => write!(self, "{number}"),
            },
            value => write!(self, "{value}"),
        }
    }

    fn ty(&mut self, ty: &Type, precedence: u8, depth: usize) -> fmt::Result {
        if depth > 64 {
            self.truncated = true;
            return Err(fmt::Error);
        }
        let own = match ty {
            Type::Function(_) | Type::Tool(_) | Type::Builtin(_) => 0,
            Type::Union(_) => 1,
            Type::Array(_) => 2,
            _ => 3,
        };
        let parentheses = own < precedence;
        if parentheses {
            self.write_char('(')?;
        }
        match ty {
            Type::Any => self.write_str("any")?,
            Type::String => self.write_str("string")?,
            Type::Number { integer } => {
                self.write_str(if *integer { "integer" } else { "number" })?
            }
            Type::Boolean => self.write_str("boolean")?,
            Type::Null => self.write_str("null")?,
            Type::Named(name) => self.write_str(name)?,
            Type::Literal(value) => self.literal(value, depth + 1)?,
            Type::Array(item) => {
                self.ty(item, 2, depth + 1)?;
                self.write_str("[]")?;
            }
            Type::Union(types) => {
                if types.is_empty() {
                    self.write_str("any")?;
                }
                for (index, ty) in types.iter().enumerate() {
                    if index > 0 {
                        self.write_str(" | ")?;
                    }
                    self.ty(ty, 1, depth + 1)?;
                }
            }
            Type::Tuple(tuple) => {
                self.write_char('[')?;
                for (index, item) in tuple.items.iter().enumerate() {
                    if index > 0 {
                        self.write_str(", ")?;
                    }
                    self.ty(&item.ty, if item.optional { 2 } else { 0 }, depth + 1)?;
                    if item.optional {
                        self.write_char('?')?;
                    }
                }
                if let Some(rest) = &tuple.rest {
                    if !tuple.items.is_empty() {
                        self.write_str(", ")?;
                    }
                    self.write_str("...")?;
                    self.ty(rest, 2, depth + 1)?;
                    self.write_str("[]")?;
                }
                self.write_char(']')?;
            }
            Type::Record(record) => {
                self.write_char('{')?;
                for (index, field) in record.fields.iter().enumerate() {
                    if index > 0 {
                        self.write_str(", ")?;
                    }
                    self.key(&field.name, false)?;
                    if field.optional {
                        self.write_char('?')?;
                    }
                    self.write_str(": ")?;
                    self.ty(&field.ty, 0, depth + 1)?;
                }
                if let Some(additional) = &record.additional {
                    if !record.fields.is_empty() {
                        self.write_str(", ")?;
                    }
                    self.write_str("[key: string]: ")?;
                    self.ty(additional, 0, depth + 1)?;
                }
                self.write_char('}')?;
            }
            Type::Server(tools) => {
                self.write_char('{')?;
                for (index, (name, ty)) in tools.iter().enumerate() {
                    if index > 0 {
                        self.write_str(", ")?;
                    }
                    self.key(name, false)?;
                    self.write_str(": ")?;
                    self.ty(ty, 0, depth + 1)?;
                }
                self.write_char('}')?;
            }
            Type::Function(signature) => self.signature(signature, depth)?,
            Type::Builtin(builtin) => self.signature(&builtin.signature, depth)?,
            Type::Tool(tool) => self.signature(&tool.signature, depth)?,
        }
        if parentheses {
            self.write_char(')')?;
        }
        Ok(())
    }
}

impl Write for TypeWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.truncated {
            return Err(fmt::Error);
        }
        let mut remaining = self.space.after(&self.out);
        for ch in text.chars() {
            if remaining.bytes < ch.len_utf8() || remaining.chars == 0 {
                self.truncated = true;
                return Err(fmt::Error);
            }
            self.out.push(ch);
            remaining.bytes -= ch.len_utf8();
            remaining.chars -= 1;
        }
        Ok(())
    }
}
