use std::{collections::HashMap, fmt};

use regex::RegexBuilder;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::{
    limits::{Budget, MAX_DEPTH, MAX_ITEMS, limit, runtime},
    value::{Value, ValueKind, equal},
};
use crate::{
    frontend::{Diagnostic, Span},
    types::{TupleItem, TupleType, Type},
};

type Result<T> = std::result::Result<T, Diagnostic>;

fn arity(name: &str, args: &[Value], min: usize, max: usize, span: Span) -> Result<()> {
    if (min..=max).contains(&args.len()) {
        return Ok(());
    }
    let expected = if min == max {
        min.to_string()
    } else {
        format!("{min} or {max}")
    };
    Err(runtime(
        format!("{name} expects {expected} arguments; got {}", args.len()),
        span,
    ))
}

fn index(value: &Value, span: Span) -> Result<usize> {
    let n = value.as_number(span)?;
    if n < 0.0 || n.fract() != 0.0 || !n.is_finite() {
        return Err(runtime("expected a nonnegative integral index", span));
    }
    Ok(n as usize)
}

fn scan(budget: &mut Budget, bytes: usize, span: Span) -> Result<()> {
    budget.charge(bytes, span)
}

fn append(out: &mut String, text: &str, budget: &mut Budget, span: Span) -> Result<()> {
    let size = out
        .len()
        .checked_add(text.len())
        .ok_or_else(|| limit("string length limit (16 MB)", span))?;
    budget.string(size, span)?;
    budget.charge(text.len(), span)?;
    out.push_str(text);
    Ok(())
}

fn pair(a: Value, b: Value, budget: &mut Budget, span: Span) -> Result<Value> {
    let ty = Type::Tuple(TupleType {
        items: vec![
            TupleItem {
                ty: a.display_type().clone(),
                optional: false,
            },
            TupleItem {
                ty: b.display_type().clone(),
                optional: false,
            },
        ],
        rest: None,
    });
    Value::new(ValueKind::Array(vec![a, b]), ty, budget, span)
}

fn array_space(count: usize, budget: &mut Budget, span: Span) -> Result<()> {
    budget.items(count, span)?;
    budget.allocation(count.saturating_mul(64), span)
}

fn grow_item(count: usize, budget: &mut Budget, span: Span) -> Result<()> {
    if count > MAX_ITEMS {
        return Err(limit("collection element limit (100,000)", span));
    }
    budget.charge(1, span)
}

fn copy_string(s: &str, budget: &mut Budget, span: Span) -> Result<Value> {
    budget.string(s.len(), span)?;
    scan(budget, s.len(), span)?;
    Value::string(s.to_owned(), budget, span)
}

fn string_join<'a>(
    parts: impl IntoIterator<Item = &'a str>,
    separator: &str,
    budget: &mut Budget,
    span: Span,
) -> Result<Value> {
    let mut out = String::new();
    for (i, part) in parts.into_iter().enumerate() {
        if i != 0 {
            append(&mut out, separator, budget, span)?;
        }
        append(&mut out, part, budget, span)?;
    }
    Value::string(out, budget, span)
}

fn json_number(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = usize::from(bytes.first() == Some(&b'-'));
    match bytes.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == bytes.len()
}

// Count escapes before serialization so JSON expansion cannot exceed the string cap.
fn json_size(value: &Value, budget: &mut Budget, span: Span) -> Result<usize> {
    budget.charge(1, span)?;
    let size = match value.kind() {
        ValueKind::Null => 4,
        ValueKind::Bool(b) => {
            if *b {
                4
            } else {
                5
            }
        }
        ValueKind::Number(n) => serde_json::to_string(n)
            .map_err(|e| runtime(e.to_string(), span))?
            .len(),
        ValueKind::String(s) => quoted_size(s, budget, span)?,
        ValueKind::Array(xs) => {
            let mut n = 2 + xs.len().saturating_sub(1);
            for x in xs {
                n += json_size(x, budget, span)?;
                budget.string(n, span)?;
            }
            n
        }
        ValueKind::Object(xs) => {
            let mut n = 2 + xs.len().saturating_sub(1);
            for (key, value) in xs {
                n += quoted_size(key, budget, span)? + 1 + json_size(value, budget, span)?;
                budget.string(n, span)?;
            }
            n
        }
        _ => return Err(runtime("value contains a non-JSON capability", span)),
    };
    budget.string(size, span)?;
    Ok(size)
}

fn quoted_size(s: &str, budget: &mut Budget, span: Span) -> Result<usize> {
    scan(budget, s.len(), span)?;
    Ok(2 + s
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            '\0'..='\u{1f}' => 6,
            _ => c.len_utf8(),
        })
        .sum::<usize>())
}

fn stringify(value: &Value, plain_string: bool, budget: &mut Budget, span: Span) -> Result<Value> {
    match value.kind() {
        ValueKind::String(s) if plain_string => return copy_string(s, budget, span),
        ValueKind::Number(n) if plain_string => {
            return Value::string(
                if *n == 0.0 { "0".into() } else { n.to_string() },
                budget,
                span,
            );
        }
        ValueKind::Closure(_)
        | ValueKind::Builtin(_)
        | ValueKind::Server(_)
        | ValueKind::Tool { .. } => {
            return Value::string(value.display_type().to_string(), budget, span);
        }
        _ => (),
    }
    if !is_json(value, budget, span)? {
        return Value::string(value.display_type().to_string(), budget, span);
    }
    let size = json_size(value, budget, span)?;
    scan(budget, size, span)?;
    let mut json = value.to_json(budget, span, false)?;
    normalize_numbers(&mut json, budget, span)?;
    let text = serde_json::to_string(&json)
        .map_err(|e| runtime(format!("cannot serialize JSON: {e}"), span))?;
    Value::string(text, budget, span)
}

pub fn call(name: &str, args: Vec<Value>, budget: &mut Budget, span: Span) -> Result<Value> {
    budget.charge(1, span)?;
    let (min, max) = match name {
        "slice" | "substring" => (2, 3),
        "range" => (1, 2),
        "concat" | "zip" | "merge" | "has" | "split" | "join" | "contains" | "starts_with"
        | "ends_with" | "index_of" | "regex_match" | "regex_find_all" => (2, 2),
        "replace" | "regex_replace" => (3, 3),
        "len" | "reverse" | "unique" | "sum" | "min" | "max" | "first" | "last" | "keys"
        | "values" | "entries" | "from_entries" | "str" | "num" | "trim" | "lower" | "upper"
        | "lines" | "floor" | "ceil" | "round" | "abs" | "parse_json" | "to_json" | "text"
        | "type_of" => (1, 1),
        _ => return Err(runtime(format!("unknown builtin `{name}`"), span)),
    };
    arity(name, &args, min, max, span)?;
    let first = &args[0];
    match name {
        "len" => {
            let n = match first.kind() {
                ValueKind::String(s) => {
                    scan(budget, s.len(), span)?;
                    s.chars().count()
                }
                ValueKind::Array(xs) => xs.len(),
                ValueKind::Object(xs) => xs.len(),
                _ => return Err(runtime("len expects an array, string, or object", span)),
            };
            Value::number(n as f64, span)
        }
        "first" | "last" => {
            let xs = first.as_array(span)?;
            if name == "first" {
                xs.first()
            } else {
                xs.last()
            }
            .cloned()
            .ok_or_else(|| runtime(format!("{name} requires a nonempty array"), span))
        }
        "reverse" => {
            let xs = first.as_array(span)?;
            array_space(xs.len(), budget, span)?;
            Value::array(xs.iter().rev().cloned().collect(), budget, span)
        }
        "slice" => {
            let xs = first.as_array(span)?;
            let start = index(&args[1], span)?.min(xs.len());
            let end = if args.len() == 3 {
                index(&args[2], span)?.min(xs.len())
            } else {
                xs.len()
            };
            let end = end.max(start);
            array_space(end - start, budget, span)?;
            Value::array(xs[start..end].to_vec(), budget, span)
        }
        "concat" => {
            let xs = first.as_array(span)?;
            let ys = args[1].as_array(span)?;
            array_space(xs.len().saturating_add(ys.len()), budget, span)?;
            Value::array(xs.iter().chain(ys).cloned().collect(), budget, span)
        }
        "unique" => {
            let xs = first.as_array(span)?;
            array_space(xs.len(), budget, span)?;
            let mut out = Vec::new();
            for x in xs {
                super::value::require_json(x, budget, span)?;
                let mut found = false;
                for y in &out {
                    if equal(x, y, budget, span)? {
                        found = true;
                        break;
                    }
                }
                if !found {
                    out.push(x.clone());
                }
            }
            Value::array(out, budget, span)
        }
        "zip" => {
            let xs = first.as_array(span)?;
            let ys = args[1].as_array(span)?;
            array_space(xs.len().min(ys.len()), budget, span)?;
            let mut out = Vec::with_capacity(xs.len().min(ys.len()));
            for (x, y) in xs.iter().zip(ys) {
                out.push(pair(x.clone(), y.clone(), budget, span)?);
            }
            Value::array(out, budget, span)
        }
        "range" => {
            let (start, end) = if args.len() == 1 {
                (0.0, first.as_number(span)?)
            } else {
                (first.as_number(span)?, args[1].as_number(span)?)
            };
            for n in [start, end] {
                if !n.is_finite() || n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0 {
                    return Err(runtime("range bounds must be safe integers", span));
                }
            }
            if args.len() == 1 && end < 0.0 {
                return Err(runtime("range count must be nonnegative", span));
            }
            let count = (end - start).max(0.0) as usize;
            array_space(count, budget, span)?;
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                out.push(Value::number(start + i as f64, span)?);
            }
            Value::array(out, budget, span)
        }
        "sum" | "min" | "max" => {
            let xs = first.as_array(span)?;
            scan(budget, xs.len(), span)?;
            if xs.is_empty() && name != "sum" {
                return Err(runtime(format!("{name} requires a nonempty array"), span));
            }
            let mut n = if name == "min" {
                f64::INFINITY
            } else if name == "max" {
                f64::NEG_INFINITY
            } else {
                0.0
            };
            for x in xs {
                let x = x.as_number(span)?;
                n = match name {
                    "min" => n.min(x),
                    "max" => n.max(x),
                    _ => n + x,
                };
                if !n.is_finite() {
                    return Err(runtime("non-finite numeric result", span));
                }
            }
            Value::number(n, span)
        }
        "keys" | "values" | "entries" => {
            let xs = first.as_object(span)?;
            array_space(xs.len(), budget, span)?;
            let mut out = Vec::with_capacity(xs.len());
            for (k, v) in xs {
                out.push(match name {
                    "keys" => copy_string(k, budget, span)?,
                    "values" => v.clone(),
                    _ => pair(copy_string(k, budget, span)?, v.clone(), budget, span)?,
                });
            }
            Value::array(out, budget, span)
        }
        "from_entries" | "merge" => {
            let mut out: Vec<(String, Value)> = Vec::new();
            let mut positions = HashMap::new();
            if name == "from_entries" {
                let entries = first.as_array(span)?;
                array_space(entries.len(), budget, span)?;
                for entry in entries {
                    let pair = entry.as_array(span)?;
                    if pair.len() != 2 {
                        return Err(runtime("from_entries expects [string, value] pairs", span));
                    }
                    put(
                        &mut out,
                        &mut positions,
                        pair[0].as_str(span)?,
                        &pair[1],
                        budget,
                        span,
                    )?;
                }
            } else {
                let a = first.as_object(span)?;
                let b = args[1].as_object(span)?;
                scan(budget, a.len() + b.len(), span)?;
                for (k, v) in a.iter().chain(b) {
                    put(&mut out, &mut positions, k, v, budget, span)?;
                }
            }
            Value::object(out, budget, span)
        }
        "has" => {
            let xs = first.as_object(span)?;
            let key = args[1].as_str(span)?;
            let mut found = false;
            for (k, _) in xs {
                scan(budget, 1 + k.len().min(key.len()), span)?;
                if k == key {
                    found = true;
                    break;
                }
            }
            Ok(Value::boolean(found))
        }
        "str" | "to_json" => stringify(first, name == "str", budget, span),
        "type_of" => Value::string(first.display_type().to_string(), budget, span),
        "num" => {
            let s = first.as_str(span)?;
            scan(budget, s.len(), span)?;
            if !json_number(s) {
                return Err(runtime(
                    "num expects a string matching JSON number grammar",
                    span,
                ));
            }
            Value::number(
                s.parse::<f64>()
                    .map_err(|_| runtime("invalid number", span))?,
                span,
            )
        }
        "split" | "lines" => {
            let s = first.as_str(span)?;
            scan(budget, s.len(), span)?;
            let mut out = Vec::new();
            if name == "lines" {
                if !s.is_empty() {
                    let mut iter = s.split('\n').peekable();
                    while let Some(part) = iter.next() {
                        if part.is_empty() && iter.peek().is_none() && s.ends_with('\n') {
                            break;
                        }
                        let part = if iter.peek().is_some() {
                            part.strip_suffix('\r').unwrap_or(part)
                        } else {
                            part
                        };
                        grow_item(out.len() + 1, budget, span)?;
                        out.push(copy_string(part, budget, span)?);
                    }
                }
            } else {
                let separator = args[1].as_str(span)?;
                scan(budget, separator.len(), span)?;
                if separator.is_empty() {
                    for c in s.chars() {
                        grow_item(out.len() + 1, budget, span)?;
                        out.push(Value::string(c.to_string(), budget, span)?);
                    }
                } else {
                    for part in s.split(separator) {
                        grow_item(out.len() + 1, budget, span)?;
                        out.push(copy_string(part, budget, span)?);
                    }
                }
            }
            Value::array(out, budget, span)
        }
        "join" => {
            let xs = first.as_array(span)?;
            let separator = args[1].as_str(span)?;
            scan(budget, xs.len(), span)?;
            let strings = xs
                .iter()
                .map(|v| v.as_str(span))
                .collect::<Result<Vec<_>>>()?;
            string_join(strings, separator, budget, span)
        }
        "trim" | "lower" | "upper" => {
            let s = first.as_str(span)?;
            scan(budget, s.len(), span)?;
            if name == "trim" {
                return copy_string(s.trim(), budget, span);
            }
            let size: usize = if name == "lower" {
                s.chars()
                    .flat_map(char::to_lowercase)
                    .map(char::len_utf8)
                    .sum()
            } else {
                s.chars()
                    .flat_map(char::to_uppercase)
                    .map(char::len_utf8)
                    .sum()
            };
            budget.string(size, span)?;
            scan(budget, size, span)?;
            Value::string(
                if name == "lower" {
                    s.to_lowercase()
                } else {
                    s.to_uppercase()
                },
                budget,
                span,
            )
        }
        "contains" => {
            if let ValueKind::String(s) = first.kind() {
                let part = args[1].as_str(span)?;
                scan(budget, s.len().saturating_add(part.len()), span)?;
                return Ok(Value::boolean(s.contains(part)));
            }
            let xs = first.as_array(span)?;
            super::value::require_json(&args[1], budget, span)?;
            let mut found = false;
            for x in xs {
                // Validate every element, including elements after a matching value.
                if equal(x, &args[1], budget, span)? {
                    found = true;
                }
            }
            Ok(Value::boolean(found))
        }
        "starts_with" | "ends_with" | "index_of" => {
            let s = first.as_str(span)?;
            let part = args[1].as_str(span)?;
            scan(budget, s.len().saturating_add(part.len()), span)?;
            match name {
                "starts_with" => Ok(Value::boolean(s.starts_with(part))),
                "ends_with" => Ok(Value::boolean(s.ends_with(part))),
                _ => Value::number(
                    s.find(part).map_or(-1.0, |i| s[..i].chars().count() as f64),
                    span,
                ),
            }
        }
        "replace" => {
            let s = first.as_str(span)?;
            let from = args[1].as_str(span)?;
            let to = args[2].as_str(span)?;
            if from.is_empty() {
                return Err(runtime("replace requires a nonempty search string", span));
            }
            scan(budget, s.len().saturating_add(from.len()), span)?;
            let mut out = String::new();
            let mut offset = 0;
            for (i, matched) in s.match_indices(from) {
                append(&mut out, &s[offset..i], budget, span)?;
                append(&mut out, to, budget, span)?;
                offset = i + matched.len();
            }
            append(&mut out, &s[offset..], budget, span)?;
            Value::string(out, budget, span)
        }
        "substring" => {
            let s = first.as_str(span)?;
            let start = index(&args[1], span)?;
            let end = if args.len() == 3 {
                index(&args[2], span)?
            } else {
                usize::MAX
            };
            scan(budget, s.len(), span)?;
            let start_byte = s.char_indices().nth(start).map_or(s.len(), |(i, _)| i);
            let end_byte = s
                .char_indices()
                .nth(end)
                .map_or(s.len(), |(i, _)| i)
                .max(start_byte);
            copy_string(&s[start_byte..end_byte], budget, span)
        }
        "regex_match" | "regex_find_all" | "regex_replace" => regex_call(name, &args, budget, span),
        "floor" | "ceil" | "round" | "abs" => {
            let n = first.as_number(span)?;
            Value::number(
                match name {
                    "floor" => n.floor(),
                    "ceil" => n.ceil(),
                    "round" => n.round(),
                    _ => n.abs(),
                },
                span,
            )
        }
        "parse_json" => parse_json(first.as_str(span)?, budget, span),
        "text" => {
            let xs = first.as_array(span)?;
            scan(budget, xs.len(), span)?;
            let mut parts = Vec::new();
            for block in xs {
                let fields = block.as_object(span)?;
                scan(budget, fields.len(), span)?;
                let kind = fields
                    .iter()
                    .find(|(k, _)| k == "type")
                    .ok_or_else(|| runtime("content block is missing type", span))?
                    .1
                    .as_str(span)?;
                if kind == "text" {
                    let text = fields
                        .iter()
                        .find(|(k, _)| k == "text")
                        .ok_or_else(|| runtime("text content block is missing text", span))?
                        .1
                        .as_str(span)?;
                    parts.push(text);
                }
            }
            string_join(parts, "\n", budget, span)
        }
        _ => unreachable!(),
    }
}

fn put(
    out: &mut Vec<(String, Value)>,
    positions: &mut HashMap<String, usize>,
    key: &str,
    value: &Value,
    budget: &mut Budget,
    span: Span,
) -> Result<()> {
    scan(budget, key.len() + 1, span)?;
    if let Some(&i) = positions.get(key) {
        out[i].1 = value.clone();
    } else {
        grow_item(out.len() + 1, budget, span)?;
        positions.insert(key.to_owned(), out.len());
        out.push((key.to_owned(), value.clone()));
    }
    Ok(())
}

fn regex_call(name: &str, args: &[Value], budget: &mut Budget, span: Span) -> Result<Value> {
    let s = args[0].as_str(span)?;
    let pattern = args[1].as_str(span)?;
    let replacement = if name == "regex_replace" {
        args[2].as_str(span)?
    } else {
        ""
    };
    scan(budget, pattern.len().saturating_mul(32), span)?;
    let re = RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .nest_limit(64)
        .build()
        .map_err(|e| runtime(format!("invalid regex: {e}"), span))?;
    let weight = pattern.len().max(1);
    if name == "regex_match" {
        scan(
            budget,
            weight.saturating_mul(s.len().saturating_add(1)),
            span,
        )?;
        return Ok(Value::boolean(re.is_match(s)));
    }
    let mut matches = re.captures_iter(s);
    let mut output = Vec::new();
    let mut text = String::new();
    let mut offset = 0;
    let mut retained = 64usize;
    loop {
        // Each search is linear; repeated searches are charged before entering the engine.
        scan(budget, weight.saturating_mul(s.len() - offset + 1), span)?;
        let Some(captures) = matches.next() else {
            break;
        };
        let matched = captures.get(0).expect("regex always has a full match");
        if name == "regex_replace" {
            append(&mut text, &s[offset..matched.start()], budget, span)?;
            append(&mut text, replacement, budget, span)?;
        } else {
            grow_item(output.len() + 1, budget, span)?;
            array_space(captures.len(), budget, span)?;
            let mut groups = Vec::with_capacity(captures.len());
            for capture in captures.iter() {
                groups.push(copy_string(
                    capture.map_or("", |m| m.as_str()),
                    budget,
                    span,
                )?);
            }
            let row = Value::array(groups, budget, span)?;
            retained = retained
                .saturating_add(row.logical_bytes())
                .saturating_add(8);
            budget.allocation(retained, span)?;
            output.push(row);
        }
        offset = matched.end();
    }
    if name == "regex_replace" {
        append(&mut text, &s[offset..], budget, span)?;
        Value::string(text, budget, span)
    } else {
        Value::array(output, budget, span)
    }
}

struct JsonState<'a> {
    budget: &'a mut Budget,
    span: Span,
    bytes: usize,
    error: Option<Diagnostic>,
}
impl JsonState<'_> {
    fn checked<T, E: de::Error>(&mut self, result: Result<T>) -> std::result::Result<T, E> {
        result.map_err(|error| {
            self.error = Some(error);
            E::custom("JSON value limit exceeded")
        })
    }
    fn finish<E: de::Error>(&mut self, value: Value) -> std::result::Result<Value, E> {
        self.bytes = self.bytes.saturating_add(value.0.own_bytes);
        let result = self.budget.allocation(self.bytes, self.span);
        self.checked(result)?;
        Ok(value)
    }
}
struct JsonSeed<'a, 'b> {
    state: &'a mut JsonState<'b>,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for JsonSeed<'_, '_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.depth > MAX_DEPTH {
            return self.state.checked(Err(limit(
                "value nesting depth limit (64)",
                self.state.span,
            )));
        }
        let result = self.state.budget.charge(1, self.state.span);
        self.state.checked(result)?;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for JsonSeed<'_, '_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        self.state.finish(Value::null())
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        self.state.finish(Value::boolean(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        self.visit_f64(v as f64)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        self.visit_f64(v as f64)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        let result = Value::number(v, self.state.span);
        let value = self.state.checked(result)?;
        self.state.finish(value)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        let result = copy_string(v, self.state.budget, self.state.span);
        let value = self.state.checked(result)?;
        self.state.finish(value)
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        self.visit_str(&v)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(value) = seq.next_element_seed(JsonSeed {
            state: self.state,
            depth: self.depth + 1,
        })? {
            let result = grow_item(out.len() + 1, self.state.budget, self.state.span);
            self.state.checked(result)?;
            out.push(value);
        }
        let result = Value::array(out, self.state.budget, self.state.span);
        let value = self.state.checked(result)?;
        self.state.finish(value)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut out = Vec::new();
        let mut positions = HashMap::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(JsonSeed {
                state: self.state,
                depth: self.depth + 1,
            })?;
            let result = put(
                &mut out,
                &mut positions,
                &key,
                &value,
                self.state.budget,
                self.state.span,
            );
            self.state.checked(result)?;
        }
        let result = Value::object(out, self.state.budget, self.state.span);
        let value = self.state.checked(result)?;
        self.state.finish(value)
    }
}

fn parse_json(s: &str, budget: &mut Budget, span: Span) -> Result<Value> {
    scan(budget, s.len(), span)?;
    let mut state = JsonState {
        budget,
        span,
        bytes: 0,
        error: None,
    };
    let mut deserializer = serde_json::Deserializer::from_str(s);
    let result = JsonSeed {
        state: &mut state,
        depth: 0,
    }
    .deserialize(&mut deserializer);
    let result = result.and_then(|value| deserializer.end().map(|()| value));
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(state
            .error
            .unwrap_or_else(|| runtime(format!("invalid JSON: {error}"), span))),
    }
}

fn normalize_numbers(value: &mut serde_json::Value, budget: &mut Budget, span: Span) -> Result<()> {
    budget.charge(1, span)?;
    match value {
        serde_json::Value::Number(number) => {
            if let Some(n) = number.as_f64()
                && n.fract() == 0.0
            {
                if n >= i64::MIN as f64 && n < (i64::MAX as f64) {
                    *number = (n as i64).into();
                } else if n >= 0.0 && n < (u64::MAX as f64) {
                    *number = (n as u64).into();
                }
            }
        }
        serde_json::Value::Array(xs) => {
            for x in xs {
                normalize_numbers(x, budget, span)?;
            }
        }
        serde_json::Value::Object(xs) => {
            for x in xs.values_mut() {
                normalize_numbers(x, budget, span)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn is_json(value: &Value, budget: &mut Budget, span: Span) -> Result<bool> {
    budget.charge(1, span)?;
    match value.kind() {
        ValueKind::Array(xs) => {
            for x in xs {
                if !is_json(x, budget, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ValueKind::Object(xs) => {
            for (_, x) in xs {
                if !is_json(x, budget, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ValueKind::Null | ValueKind::Bool(_) | ValueKind::Number(_) | ValueKind::String(_) => {
            Ok(true)
        }
        _ => Ok(false),
    }
}
