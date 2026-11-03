use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};
use url::Url;

use super::{Field, RecordType, TupleItem, TupleType, Type, union};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationNote {
    /// JSON Pointer into the original schema.
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivedType {
    pub ty: Type,
    pub definitions: BTreeMap<String, Type>,
    pub notes: Vec<DerivationNote>,
}

impl DerivedType {
    pub fn render_definitions(&self) -> String {
        self.definitions
            .iter()
            .map(|(name, ty)| format!("{name} = {ty}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Derive without I/O. Unsupported schemas remain discoverable and carry notes.
pub fn derive_schema(schema: &Value) -> DerivedType {
    if !schema_within_budget(schema) {
        return DerivedType {
            ty: Type::Any,
            definitions: BTreeMap::new(),
            notes: vec![DerivationNote {
                path: String::new(),
                message: "schema size/depth budget exceeded; using any".into(),
            }],
        };
    }
    let mut context = Context {
        root: schema,
        scopes: HashMap::new(),
        resources: HashMap::new(),
        anchors: HashMap::new(),
        active: HashSet::new(),
        cache: HashMap::new(),
        names: BTreeMap::new(),
        notes: Vec::new(),
        remaining: MAX_NODES,
    };
    let base = Url::parse("mcpl://schema/root").expect("static URI");
    context.resources.insert(base.to_string(), String::new());
    context.index(schema, "", &base, 0);
    context.remaining = MAX_NODES;
    let ty = context.derive("", 0);
    let definitions = context
        .names
        .iter()
        .map(|(path, name)| {
            (
                name.clone(),
                context.cache.get(path).cloned().unwrap_or(Type::Any),
            )
        })
        .collect();
    DerivedType {
        ty,
        definitions,
        notes: context.notes,
    }
}

struct Context<'a> {
    root: &'a Value,
    scopes: HashMap<String, Url>,
    resources: HashMap<String, String>,
    anchors: HashMap<String, String>,
    active: HashSet<String>,
    cache: HashMap<String, Type>,
    names: BTreeMap<String, String>,
    notes: Vec<DerivationNote>,
    remaining: usize,
}

fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

impl Context<'_> {
    fn note(&mut self, path: &str, message: impl Into<String>) {
        let note = DerivationNote {
            path: path.into(),
            message: message.into(),
        };
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    fn fallback(&mut self, path: &str, message: &str) -> Type {
        self.note(path, message);
        Type::Any
    }

    fn index(&mut self, schema: &Value, path: &str, inherited: &Url, depth: usize) {
        if depth > MAX_DEPTH || self.remaining == 0 {
            self.note(path, "schema indexing budget exceeded");
            return;
        }
        self.remaining -= 1;
        let mut base = inherited.clone();
        if let Some(id) = schema
            .get("$id")
            .or_else(|| schema.get("id"))
            .and_then(Value::as_str)
        {
            match base.join(id) {
                Ok(uri) => {
                    base = uri;
                    if base.fragment().is_some() {
                        self.anchors.insert(base.to_string(), path.into());
                    } else {
                        self.resources.insert(base.to_string(), path.into());
                    }
                }
                Err(_) => self.note(path, "invalid schema identifier ignored"),
            }
        }
        self.scopes.insert(path.into(), base.clone());
        if let Some(anchor) = schema.get("$anchor").and_then(Value::as_str) {
            let mut uri = base.clone();
            uri.set_fragment(Some(anchor));
            self.anchors.insert(uri.to_string(), path.into());
        }
        let Some(object) = schema.as_object() else {
            return;
        };
        for (key, value) in object {
            let p = child(path, key);
            match key.as_str() {
                "$defs" | "definitions" | "properties" | "patternProperties"
                | "dependentSchemas" => {
                    if let Some(map) = value.as_object() {
                        for (key, value) in map {
                            self.index(value, &child(&p, key), &base, depth + 1);
                        }
                    }
                }
                "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                    if let Some(items) = value.as_array() {
                        for (i, item) in items.iter().enumerate() {
                            self.index(item, &child(&p, &i.to_string()), &base, depth + 1);
                        }
                    }
                }
                "items" if value.is_array() => {
                    for (i, item) in value.as_array().unwrap().iter().enumerate() {
                        self.index(item, &child(&p, &i.to_string()), &base, depth + 1);
                    }
                }
                "items"
                | "additionalItems"
                | "additionalProperties"
                | "contains"
                | "not"
                | "if"
                | "then"
                | "else"
                | "propertyNames"
                | "unevaluatedItems"
                | "unevaluatedProperties"
                | "contentSchema" => self.index(value, &p, &base, depth + 1),
                _ => {}
            }
        }
    }

    fn name(&mut self, path: &str) -> String {
        if let Some(name) = self.names.get(path) {
            return name.clone();
        }
        let preferred = if path.is_empty() {
            "Root".into()
        } else {
            path.rsplit('/')
                .next()
                .unwrap_or("Type")
                .replace("~1", "/")
                .replace("~0", "~")
        };
        let mut stem: String = preferred
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if stem.is_empty() || stem.starts_with(|c: char| c.is_ascii_digit()) {
            stem.insert_str(0, "Type_");
        }
        if stem == "Content" {
            stem = "SchemaContent".into();
        }
        let mut name = stem.clone();
        let mut n = 2;
        while self.names.values().any(|existing| existing == &name) {
            name = format!("{stem}_{n}");
            n += 1;
        }
        self.names.insert(path.into(), name.clone());
        name
    }

    fn derive(&mut self, path: &str, depth: usize) -> Type {
        if depth > MAX_DEPTH || self.remaining == 0 {
            return self.fallback(path, "schema derivation budget exceeded; using any");
        }
        self.remaining -= 1;
        if self.active.contains(path) {
            return Type::Named(self.name(path));
        }
        if let Some(ty) = self.cache.get(path) {
            let cost = type_size(ty);
            if cost > self.remaining {
                return self.fallback(path, "schema expansion budget exceeded; using any");
            }
            self.remaining -= cost;
            return ty.clone();
        }
        self.active.insert(path.into());
        let root = self.root;
        let schema = root.pointer(path).unwrap_or(&Value::Null);
        let ty = match schema {
            Value::Bool(true) => Type::Any,
            Value::Bool(false) => self.fallback(
                path,
                "false schema has no representable bottom type; using any",
            ),
            Value::Object(object) => self.object_schema(object, path, depth),
            _ => self.fallback(path, "malformed schema; expected object or boolean"),
        };
        self.active.remove(path);
        self.cache.insert(path.into(), ty.clone());
        ty
    }

    fn reference(&mut self, reference: &str, path: &str, depth: usize) -> Type {
        let Some(base) = self.scopes.get(path) else {
            return self.fallback(path, "reference scope unavailable; using any");
        };
        let Ok(mut uri) = base.join(reference) else {
            return self.fallback(path, "malformed reference; using any");
        };
        let target = if let Some(target) = self.anchors.get(uri.as_str()) {
            Some(target.clone())
        } else {
            let fragment = uri.fragment().unwrap_or("").to_owned();
            uri.set_fragment(None);
            self.resources.get(uri.as_str()).and_then(|resource| {
                let pointer = percent_decode(&fragment)?;
                if !pointer.is_empty() && !pointer.starts_with('/') {
                    return None;
                }
                let target = format!("{resource}{pointer}");
                self.root.pointer(&target).map(|_| target)
            })
        };
        match target {
            Some(target) => self.derive(&target, depth + 1),
            None => self.fallback(
                path,
                &format!("unresolved reference {reference:?}; using any (no external retrieval)"),
            ),
        }
    }

    fn combine(&mut self, a: Type, b: Type, path: &str) -> Type {
        match intersect(a, b) {
            Some(ty) => ty,
            None => self.fallback(path, "intersection is not representable; using any"),
        }
    }

    fn object_schema(&mut self, object: &Map<String, Value>, path: &str, depth: usize) -> Type {
        let mut ty = match object.get("type") {
            None => Type::Any,
            Some(Value::String(name)) => self.kind(name, object, path, depth),
            Some(Value::Array(names)) if !names.is_empty() => union(
                names
                    .iter()
                    .map(|name| match name.as_str() {
                        Some(name) => self.kind(name, object, path, depth),
                        None => self.fallback(
                            &child(path, "type"),
                            "malformed type array member; using any",
                        ),
                    })
                    .collect::<Vec<_>>(),
            ),
            Some(_) => self.fallback(&child(path, "type"), "malformed type keyword; using any"),
        };
        if let Some(value) = object.get("const") {
            ty = self.combine(ty, literal_shape(value), path);
        }
        if let Some(values) = object.get("enum") {
            let enumerated = match values.as_array().filter(|v| !v.is_empty()) {
                Some(values) => union(values.iter().map(literal_shape)),
                None => self.fallback(&child(path, "enum"), "empty or malformed enum; using any"),
            };
            ty = self.combine(ty, enumerated, path);
        }
        for keyword in ["anyOf", "oneOf", "allOf"] {
            if let Some(value) = object.get(keyword) {
                let p = child(path, keyword);
                let Some(branches) = value.as_array().filter(|v| !v.is_empty()) else {
                    return self.fallback(&p, "empty or malformed applicator; using any");
                };
                let mut derived = Vec::new();
                for i in 0..branches.len() {
                    derived.push(self.derive(&child(&p, &i.to_string()), depth + 1));
                }
                let branch_type = if keyword == "allOf" {
                    let mut merged = Type::Any;
                    for branch in derived {
                        merged = self.combine(merged, branch, &p);
                    }
                    merged
                } else {
                    union(derived)
                };
                ty = self.combine(ty, branch_type, path);
            }
        }
        if let Some(reference) = object.get("$ref") {
            let target = match reference.as_str() {
                Some(reference) => self.reference(reference, path, depth),
                None => self.fallback(path, "malformed $ref; using any"),
            };
            // Drafts before 2019-09 ignore siblings of $ref.
            if self.legacy_ref(path) {
                return target;
            }
            ty = self.combine(ty, target, path);
        }
        if object.contains_key("if") {
            let yes = if object.contains_key("then") {
                self.derive(&child(path, "then"), depth + 1)
            } else {
                Type::Any
            };
            let no = if object.contains_key("else") {
                self.derive(&child(path, "else"), depth + 1)
            } else {
                Type::Any
            };
            ty = self.combine(ty, union([yes, no]), path);
            self.note(
                path,
                "conditional branches approximated; condition checked at call boundary",
            );
        }
        if object.contains_key("not") {
            self.note(path, "negation checked only at call boundary");
        }
        for key in object.keys() {
            if !KNOWN_KEYWORDS.contains(&key.as_str()) {
                self.note(
                    &child(path, key),
                    "unsupported keyword; retaining only independently known type information",
                );
            }
        }
        if object.contains_key("$dynamicRef") || object.contains_key("$recursiveRef") {
            self.note(path, "dynamic reference not resolved; retaining only independently known type information");
        }
        if object.get("nullable") == Some(&Value::Bool(true)) {
            ty = union([ty, Type::Null]);
        }
        ty
    }

    fn legacy_ref(&self, path: &str) -> bool {
        let mut p = path;
        loop {
            if let Some(dialect) = self
                .root
                .pointer(p)
                .and_then(|s| s.get("$schema"))
                .and_then(Value::as_str)
            {
                return ["draft-04", "draft-06", "draft-07"]
                    .iter()
                    .any(|old| dialect.contains(old));
            }
            let Some((parent, _)) = p.rsplit_once('/') else {
                return false;
            };
            p = parent;
        }
    }

    fn kind(&mut self, name: &str, object: &Map<String, Value>, path: &str, depth: usize) -> Type {
        match name {
            "string" => Type::String,
            "number" => Type::number(),
            "integer" => Type::Number { integer: true },
            "boolean" => Type::Boolean,
            "null" => Type::Null,
            "object" => self.record(object, path, depth),
            "array" => self.array(object, path, depth),
            _ => self.fallback(path, &format!("unknown type {name:?}; using any")),
        }
    }

    fn record(&mut self, object: &Map<String, Value>, path: &str, depth: usize) -> Type {
        let additional = match object.get("additionalProperties") {
            Some(Value::Bool(false)) => None,
            None | Some(Value::Bool(true)) => Some(Type::Any),
            Some(_) => Some(self.derive(&child(path, "additionalProperties"), depth + 1)),
        };
        let mut dynamic = additional.clone().into_iter().collect::<Vec<_>>();
        if let Some(patterns) = object.get("patternProperties") {
            if let Some(patterns) = patterns.as_object() {
                for key in patterns.keys() {
                    dynamic.push(
                        self.derive(&child(&child(path, "patternProperties"), key), depth + 1),
                    );
                }
                if !patterns.is_empty() {
                    self.note(
                        path,
                        "pattern keys approximated by a dynamic index signature",
                    );
                }
            } else {
                dynamic.push(self.fallback(
                    path,
                    "malformed patternProperties; using any dynamic values",
                ));
            }
        }
        let dynamic = if dynamic.is_empty() {
            None
        } else {
            Some(Box::new(union(dynamic)))
        };
        let required: Vec<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if object.get("required").is_some_and(|v| {
            v.as_array()
                .is_none_or(|a| a.iter().any(|v| !v.is_string()))
        }) {
            self.note(path, "malformed required keyword ignored");
        }
        let mut fields = Vec::new();
        if let Some(properties) = object.get("properties") {
            if let Some(properties) = properties.as_object() {
                for name in properties.keys() {
                    fields.push(Field {
                        name: name.clone(),
                        ty: self.derive(&child(&child(path, "properties"), name), depth + 1),
                        optional: !required.contains(&name.as_str()),
                    });
                }
            } else {
                return self.fallback(path, "malformed properties; using any");
            }
        }
        for name in required {
            if !fields.iter().any(|field| field.name == name) {
                fields.push(Field {
                    name: name.into(),
                    ty: dynamic.as_deref().cloned().unwrap_or_else(|| {
                        self.note(
                            path,
                            "undeclared required field in closed record; using any for field",
                        );
                        Type::Any
                    }),
                    optional: false,
                });
            }
        }
        if object.contains_key("unevaluatedProperties") {
            self.note(path, "unevaluatedProperties checked at call boundary; additional-key approximation retained");
        }
        Type::Record(RecordType {
            fields,
            additional: dynamic,
        })
    }

    fn array(&mut self, object: &Map<String, Value>, path: &str, depth: usize) -> Type {
        let prefix_key = if object.contains_key("prefixItems") {
            Some("prefixItems")
        } else if object.get("items").is_some_and(Value::is_array) {
            Some("items")
        } else {
            None
        };
        if object.contains_key("unevaluatedItems") {
            self.note(
                path,
                "unevaluatedItems checked at call boundary; tail approximation retained",
            );
        }
        if let Some(key) = prefix_key {
            let Some(prefix) = object[key].as_array() else {
                return self.fallback(path, "malformed tuple prefix; using any");
            };
            let min = object.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            let items = (0..prefix.len())
                .map(|i| TupleItem {
                    ty: self.derive(&child(&child(path, key), &i.to_string()), depth + 1),
                    optional: i as u64 >= min,
                })
                .collect();
            let tail_key = if key == "items" {
                "additionalItems"
            } else {
                "items"
            };
            let rest = match object.get(tail_key) {
                Some(Value::Bool(false)) => None,
                None | Some(Value::Bool(true)) => Some(Box::new(Type::Any)),
                Some(_) => Some(Box::new(self.derive(&child(path, tail_key), depth + 1))),
            };
            Type::Tuple(TupleType { items, rest })
        } else {
            match object.get("items") {
                Some(Value::Bool(false)) => Type::Tuple(TupleType {
                    items: vec![],
                    rest: None,
                }),
                None | Some(Value::Bool(true)) => Type::Array(Box::new(Type::Any)),
                Some(_) => Type::Array(Box::new(self.derive(&child(path, "items"), depth + 1))),
            }
        }
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut input = text.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let a = (input.next()? as char).to_digit(16)?;
            let b = (input.next()? as char).to_digit(16)?;
            bytes.push((a * 16 + b) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

fn literal_shape(value: &Value) -> Type {
    match value {
        Value::Null => Type::Null,
        Value::Array(values) => Type::Tuple(TupleType {
            items: values
                .iter()
                .map(|v| TupleItem {
                    ty: literal_shape(v),
                    optional: false,
                })
                .collect(),
            rest: None,
        }),
        Value::Object(values) => Type::Record(RecordType {
            fields: values
                .iter()
                .map(|(name, value)| Field {
                    name: name.clone(),
                    ty: literal_shape(value),
                    optional: false,
                })
                .collect(),
            additional: None,
        }),
        _ => Type::Literal(value.clone()),
    }
}

/// `None` means the intersection cannot be represented, not that it is empty.
fn intersect(a: Type, b: Type) -> Option<Type> {
    if a == b {
        return Some(a);
    }
    match (a, b) {
        (Type::Any, b) | (b, Type::Any) => Some(b),
        (Type::Number { integer: a }, Type::Number { integer: b }) => {
            Some(Type::Number { integer: a || b })
        }
        (Type::Union(xs), b) | (b, Type::Union(xs)) => {
            let alternatives = xs
                .into_iter()
                .filter(|a| !disjoint(a, &b))
                .map(|a| intersect(a, b.clone()))
                .collect::<Option<Vec<_>>>()?;
            if alternatives.is_empty() {
                None
            } else {
                Some(union(alternatives))
            }
        }
        (Type::Literal(v), Type::String) | (Type::String, Type::Literal(v)) if v.is_string() => {
            Some(Type::Literal(v))
        }
        (Type::Literal(v), Type::Number { .. }) | (Type::Number { .. }, Type::Literal(v))
            if v.is_number() =>
        {
            Some(Type::Literal(v))
        }
        (Type::Literal(v), Type::Boolean) | (Type::Boolean, Type::Literal(v)) if v.is_boolean() => {
            Some(Type::Literal(v))
        }
        (Type::Array(a), Type::Array(b)) => Some(Type::Array(Box::new(intersect(*a, *b)?))),
        (Type::Tuple(mut a), Type::Array(b)) | (Type::Array(b), Type::Tuple(mut a)) => {
            for item in &mut a.items {
                item.ty = intersect(item.ty.clone(), *b.clone())?;
            }
            if let Some(rest) = a.rest {
                a.rest = Some(Box::new(intersect(*rest, *b)?));
            }
            Some(Type::Tuple(a))
        }
        (Type::Record(mut a), Type::Record(b)) => {
            // A closed side cannot admit fields declared only by the other side.
            if (a.additional.is_none()
                && b.fields
                    .iter()
                    .any(|f| !a.fields.iter().any(|g| f.name == g.name)))
                || (b.additional.is_none()
                    && a.fields
                        .iter()
                        .any(|f| !b.fields.iter().any(|g| f.name == g.name)))
            {
                return None;
            }
            for field in &mut a.fields {
                if let Some(other) = b.fields.iter().find(|f| f.name == field.name) {
                    field.ty = intersect(field.ty.clone(), other.ty.clone())?;
                    field.optional &= other.optional;
                } else if let Some(extra) = &b.additional {
                    field.ty = intersect(field.ty.clone(), *extra.clone())?;
                }
            }
            for mut field in b.fields {
                if !a.fields.iter().any(|f| f.name == field.name) {
                    if let Some(extra) = &a.additional {
                        field.ty = intersect(field.ty, *extra.clone())?;
                    }
                    a.fields.push(field);
                }
            }
            a.additional = match (a.additional, b.additional) {
                (Some(a), Some(b)) => Some(Box::new(intersect(*a, *b)?)),
                _ => None,
            };
            Some(Type::Record(a))
        }
        _ => None,
    }
}

fn disjoint(a: &Type, b: &Type) -> bool {
    fn kind(ty: &Type) -> Option<u8> {
        match ty {
            Type::Null => Some(0),
            Type::String => Some(1),
            Type::Number { .. } => Some(2),
            Type::Boolean => Some(3),
            Type::Array(_) | Type::Tuple(_) => Some(4),
            Type::Record(_) => Some(5),
            Type::Literal(value) => match value {
                Value::Null => Some(0),
                Value::String(_) => Some(1),
                Value::Number(_) => Some(2),
                Value::Bool(_) => Some(3),
                _ => None,
            },
            _ => None,
        }
    }
    match (a, b) {
        (Type::Union(xs), b) | (b, Type::Union(xs)) => xs.iter().all(|a| disjoint(a, b)),
        (Type::Literal(a), Type::Literal(b)) if a.is_number() && b.is_number() => {
            a.as_f64() != b.as_f64()
        }
        (Type::Literal(a), Type::Literal(b)) => a != b,
        _ => matches!((kind(a), kind(b)), (Some(a), Some(b)) if a != b),
    }
}

const KNOWN_KEYWORDS: &[&str] = &[
    "type",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "patternProperties",
    "propertyNames",
    "dependentRequired",
    "dependentSchemas",
    "dependencies",
    "items",
    "prefixItems",
    "additionalItems",
    "contains",
    "minContains",
    "maxContains",
    "uniqueItems",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "$ref",
    "$defs",
    "definitions",
    "$id",
    "id",
    "$anchor",
    "$dynamicRef",
    "$dynamicAnchor",
    "$recursiveRef",
    "$recursiveAnchor",
    "$schema",
    "$vocabulary",
    "nullable",
    "format",
    "pattern",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "minProperties",
    "maxProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "contentEncoding",
    "contentMediaType",
    "contentSchema",
    "title",
    "description",
    "default",
    "examples",
    "$comment",
    "readOnly",
    "writeOnly",
    "deprecated",
];

fn schema_within_budget(schema: &Value) -> bool {
    let mut stack = vec![(schema, 0)];
    let mut count = 0;
    while let Some((value, depth)) = stack.pop() {
        count += 1;
        if count > MAX_NODES || depth > MAX_DEPTH {
            return false;
        }
        let children = match value {
            Value::Array(a) => a.len(),
            Value::Object(o) => o.len(),
            _ => 0,
        };
        if count + stack.len() + children > MAX_NODES {
            return false;
        }
        match value {
            Value::Array(a) => stack.extend(a.iter().map(|v| (v, depth + 1))),
            Value::Object(o) => stack.extend(o.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    true
}

fn type_size(ty: &Type) -> usize {
    1 + match ty {
        Type::Array(t) => type_size(t),
        Type::Tuple(t) => {
            t.items.iter().map(|i| type_size(&i.ty)).sum::<usize>()
                + t.rest.as_deref().map(type_size).unwrap_or(0)
        }
        Type::Record(r) => {
            r.fields.iter().map(|f| type_size(&f.ty)).sum::<usize>()
                + r.additional.as_deref().map(type_size).unwrap_or(0)
        }
        Type::Union(types) => types.iter().map(type_size).sum(),
        _ => 0,
    }
}
