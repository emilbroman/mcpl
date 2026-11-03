use std::fmt;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Any,
    String,
    Number { integer: bool },
    Boolean,
    Null,
    Literal(Value),
    Array(Box<Type>),
    Tuple(TupleType),
    Record(RecordType),
    Union(Vec<Type>),
    Function(FunctionType),
    Tool(ToolType),
    Builtin(BuiltinType),
    Server(Vec<(String, Type)>),
    Named(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordType {
    pub fields: Vec<Field>,
    pub additional: Option<Box<Type>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TupleType {
    pub items: Vec<TupleItem>,
    pub rest: Option<Box<Type>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TupleItem {
    pub ty: Type,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionType {
    pub parameters: Vec<Parameter>,
    pub result: Box<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinType {
    pub name: String,
    pub signature: FunctionType,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolType {
    pub server: String,
    pub name: String,
    pub signature: FunctionType,
}

impl Type {
    pub fn number() -> Self {
        Self::Number { integer: false }
    }

    fn precedence(&self) -> u8 {
        match self {
            Self::Function(_) | Self::Tool(_) | Self::Builtin(_) => 0,
            Self::Union(_) => 1,
            Self::Array(_) => 2,
            _ => 3,
        }
    }

    fn fmt_at(&self, f: &mut fmt::Formatter<'_>, precedence: u8) -> fmt::Result {
        let parentheses = self.precedence() < precedence;
        if parentheses {
            f.write_str("(")?;
        }
        match self {
            Self::Any => f.write_str("any")?,
            Self::String => f.write_str("string")?,
            Self::Number { integer } => {
                f.write_str(if *integer { "integer" } else { "number" })?;
            }
            Self::Boolean => f.write_str("boolean")?,
            Self::Null => f.write_str("null")?,
            Self::Literal(value) => fmt_literal(value, f)?,
            Self::Array(item) => {
                item.fmt_at(f, 2)?;
                f.write_str("[]")?;
            }
            Self::Tuple(tuple) => {
                f.write_str("[")?;
                for (index, item) in tuple.items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    item.ty.fmt_at(f, if item.optional { 2 } else { 0 })?;
                    if item.optional {
                        f.write_str("?")?;
                    }
                }
                if let Some(rest) = &tuple.rest {
                    if !tuple.items.is_empty() {
                        f.write_str(", ")?;
                    }
                    f.write_str("...")?;
                    rest.fmt_at(f, 2)?;
                    f.write_str("[]")?;
                }
                f.write_str("]")?;
            }
            Self::Record(record) => {
                f.write_str("{")?;
                for (index, field) in record.fields.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    fmt_key(&field.name, f)?;
                    if field.optional {
                        f.write_str("?")?;
                    }
                    write!(f, ": {}", field.ty)?;
                }
                if let Some(additional) = &record.additional {
                    if !record.fields.is_empty() {
                        f.write_str(", ")?;
                    }
                    write!(f, "[key: string]: {additional}")?;
                }
                f.write_str("}")?;
            }
            Self::Union(types) => {
                if types.is_empty() {
                    f.write_str("any")?;
                }
                for (index, ty) in types.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" | ")?;
                    }
                    ty.fmt_at(f, 1)?;
                }
            }
            Self::Function(signature) => write!(f, "{signature}")?,
            Self::Tool(tool) => write!(f, "{}", tool.signature)?,
            Self::Builtin(builtin) => write!(f, "{}", builtin.signature)?,
            Self::Server(tools) => {
                f.write_str("{")?;
                for (index, (name, ty)) in tools.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    fmt_key(name, f)?;
                    write!(f, ": {ty}")?;
                }
                f.write_str("}")?;
            }
            Self::Named(name) => f.write_str(name)?,
        }
        if parentheses {
            f.write_str(")")?;
        }
        Ok(())
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_at(f, 0)
    }
}

impl fmt::Display for FunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (index, parameter) in self.parameters.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            fmt_key(&parameter.name, f)?;
            write!(f, ": {}", parameter.ty)?;
        }
        write!(f, ") => {}", self.result)
    }
}

fn fmt_literal(value: &Value, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Value::Number(number) = value
        && let Some(number) = number.as_f64()
    {
        return write!(f, "{}", if number == 0.0 { 0.0 } else { number });
    }
    write!(f, "{value}")
}

fn fmt_key(key: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if bare_key(key) {
        f.write_str(key)
    } else {
        write!(f, "{}", Value::String(key.to_owned()))
    }
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
    if !(first.is_ascii_alphabetic() || first == b'_') || matches!(root, "true" | "false" | "null")
    {
        return false;
    }
    std::iter::once(root).chain(parts).all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

pub fn union(types: impl IntoIterator<Item = Type>) -> Type {
    fn append(ty: Type, alternatives: &mut Vec<Type>) -> bool {
        match ty {
            Type::Any => return false,
            Type::Union(types) => {
                for ty in types {
                    if !append(ty, alternatives) {
                        return false;
                    }
                }
            }
            ty => {
                if !alternatives.contains(&ty) {
                    alternatives.push(ty);
                }
            }
        }
        true
    }

    let mut alternatives = Vec::new();
    for ty in types {
        if !append(ty, &mut alternatives) {
            return Type::Any;
        }
    }
    match alternatives.len() {
        0 => Type::Any,
        1 => alternatives.pop().unwrap(),
        _ => Type::Union(alternatives),
    }
}

pub fn content_result_type() -> Type {
    Type::Array(Box::new(Type::Named("Content".into())))
}

pub fn content_type() -> Type {
    fn field(name: &str, ty: Type, optional: bool) -> Field {
        Field {
            name: name.into(),
            ty,
            optional,
        }
    }
    fn record(fields: Vec<Field>) -> Type {
        Type::Record(RecordType {
            fields,
            additional: None,
        })
    }
    fn array(ty: Type) -> Type {
        Type::Array(Box::new(ty))
    }
    fn literal(value: &str) -> Type {
        Type::Literal(Value::String(value.into()))
    }
    let meta = Type::Record(RecordType {
        fields: Vec::new(),
        additional: Some(Box::new(Type::Any)),
    });
    let annotations = record(vec![
        field(
            "audience",
            array(union([literal("user"), literal("assistant")])),
            true,
        ),
        field("priority", Type::number(), true),
        field("lastModified", Type::String, true),
    ]);
    let icon = record(vec![
        field("src", Type::String, false),
        field("mimeType", Type::String, true),
        field("sizes", array(Type::String), true),
        field("theme", union([literal("light"), literal("dark")]), true),
    ]);
    let resource = union(["text", "blob"].map(|body| {
        record(vec![
            field("uri", Type::String, false),
            field("mimeType", Type::String, true),
            field(body, Type::String, false),
            field("_meta", meta.clone(), true),
        ])
    }));
    let content = |kind: &str, mut fields: Vec<Field>| {
        fields.insert(0, field("type", literal(kind), false));
        fields.push(field("annotations", annotations.clone(), true));
        fields.push(field("_meta", meta.clone(), true));
        record(fields)
    };
    union([
        content("text", vec![field("text", Type::String, false)]),
        content(
            "image",
            vec![
                field("data", Type::String, false),
                field("mimeType", Type::String, false),
            ],
        ),
        content(
            "audio",
            vec![
                field("data", Type::String, false),
                field("mimeType", Type::String, false),
            ],
        ),
        content("resource", vec![field("resource", resource, false)]),
        content(
            "resource_link",
            vec![
                field("name", Type::String, false),
                field("title", Type::String, true),
                field("uri", Type::String, false),
                field("description", Type::String, true),
                field("mimeType", Type::String, true),
                field("size", Type::number(), true),
                field("icons", array(icon), true),
            ],
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn signature(result: Type) -> FunctionType {
        FunctionType {
            parameters: vec![Parameter {
                name: "args".into(),
                ty: Type::Record(RecordType {
                    fields: vec![Field {
                        name: "url".into(),
                        ty: Type::String,
                        optional: false,
                    }],
                    additional: None,
                }),
            }],
            result: Box::new(result),
        }
    }

    #[test]
    fn renders_primitives_and_literals() {
        for (ty, expected) in [
            (Type::Any, "any"),
            (Type::String, "string"),
            (Type::number(), "number"),
            (Type::Number { integer: true }, "integer"),
            (Type::Boolean, "boolean"),
            (Type::Null, "null"),
            (Type::Literal(json!(42.0)), "42"),
            (Type::Literal(json!(-0.0)), "0"),
            (Type::Literal(json!(1.5)), "1.5"),
            (Type::Literal(json!(true)), "true"),
            (Type::Literal(json!("a\n\"b")), "\"a\\n\\\"b\""),
        ] {
            assert_eq!(ty.to_string(), expected);
        }
    }

    #[test]
    fn union_flattens_and_preserves_first_occurrence() {
        assert_eq!(union([]), Type::Any);
        assert_eq!(union([Type::String, Type::String]), Type::String);
        assert_eq!(
            union([Type::String, Type::Union(vec![Type::Null, Type::String])]),
            Type::Union(vec![Type::String, Type::Null])
        );
        assert_eq!(
            union([Type::Union(vec![Type::String, Type::Any])]),
            Type::Any
        );
    }

    #[test]
    fn renders_precedence_and_tuples() {
        let nullable = union([Type::String, Type::Null]);
        assert_eq!(
            Type::Array(Box::new(nullable.clone())).to_string(),
            "(string | null)[]"
        );
        let function = Type::Function(signature(Type::String));
        assert_eq!(
            Type::Array(Box::new(function.clone())).to_string(),
            "((args: {url: string}) => string)[]"
        );
        assert_eq!(
            union([function, Type::Null]).to_string(),
            "((args: {url: string}) => string) | null"
        );
        let tuple = Type::Tuple(TupleType {
            items: vec![
                TupleItem {
                    ty: Type::String,
                    optional: false,
                },
                TupleItem {
                    ty: nullable.clone(),
                    optional: true,
                },
            ],
            rest: Some(Box::new(nullable)),
        });
        assert_eq!(
            tuple.to_string(),
            "[string, (string | null)?, ...(string | null)[]]"
        );
        assert_eq!(
            Type::Tuple(TupleType {
                items: vec![],
                rest: None
            })
            .to_string(),
            "[]"
        );
    }

    #[test]
    fn renders_record_keys_and_index_signatures() {
        let keys = [
            "a.b",
            "browser-navigate",
            "true",
            "true.field",
            "2fa",
            "a..b",
            "with space",
            "quote\"",
            "\n",
            "é",
        ];
        let record = Type::Record(RecordType {
            fields: keys
                .into_iter()
                .map(|name| Field {
                    name: name.into(),
                    ty: Type::String,
                    optional: true,
                })
                .collect(),
            additional: Some(Box::new(Type::number())),
        });
        assert_eq!(
            record.to_string(),
            "{a.b?: string, browser-navigate?: string, \"true\"?: string, \"true.field\"?: string, \"2fa\"?: string, \"a..b\"?: string, \"with space\"?: string, \"quote\\\"\"?: string, \"\\n\"?: string, \"é\"?: string, [key: string]: number}"
        );
        assert!(!bare_key(&"a".repeat(129)));
        assert!(bare_key("a.2b.-b"));
        assert_eq!(
            Type::Record(RecordType {
                fields: vec![],
                additional: None
            })
            .to_string(),
            "{}"
        );
    }

    #[test]
    fn renders_tool_and_server_without_capability_identity() {
        let signature = signature(content_result_type());
        assert_eq!(signature.to_string(), "(args: {url: string}) => Content[]");
        let tool = Type::Tool(ToolType {
            server: "playwright".into(),
            name: "browser_navigate".into(),
            signature,
        });
        assert_eq!(tool.to_string(), "(args: {url: string}) => Content[]");
        assert_eq!(
            Type::Server(vec![("browser_navigate".into(), tool)]).to_string(),
            "{browser_navigate: (args: {url: string}) => Content[]}"
        );
    }

    #[test]
    fn content_has_exact_variant_order_and_fields() {
        let Type::Union(variants) = content_type() else {
            panic!("expected union")
        };
        let expected: &[(&str, &[(&str, bool)])] = &[
            ("text", &[("text", false)]),
            ("image", &[("data", false), ("mimeType", false)]),
            ("audio", &[("data", false), ("mimeType", false)]),
            ("resource", &[("resource", false)]),
            (
                "resource_link",
                &[
                    ("name", false),
                    ("title", true),
                    ("uri", false),
                    ("description", true),
                    ("mimeType", true),
                    ("size", true),
                    ("icons", true),
                ],
            ),
        ];
        assert_eq!(variants.len(), expected.len());
        for (variant, (kind, fields)) in variants.iter().zip(expected) {
            let Type::Record(record) = variant else {
                panic!("expected record")
            };
            assert_eq!(record.additional, None);
            assert_eq!(record.fields[0].ty, Type::Literal(json!(kind)));
            assert_eq!(record.fields[0].name, "type");
            assert!(!record.fields[0].optional);
            let mut expected_fields = fields.to_vec();
            expected_fields.extend([("annotations", true), ("_meta", true)]);
            assert_eq!(
                record.fields[1..]
                    .iter()
                    .map(|field| (field.name.as_str(), field.optional))
                    .collect::<Vec<_>>(),
                expected_fields
            );
        }
        assert!(variants[0].to_string().contains("annotations?: {audience?: (\"user\" | \"assistant\")[], priority?: number, lastModified?: string}"));
        assert!(variants[3].to_string().contains("resource: {uri: string, mimeType?: string, text: string, _meta?: {[key: string]: any}} | {uri: string, mimeType?: string, blob: string, _meta?: {[key: string]: any}}"));
        assert!(variants[4].to_string().contains("icons?: {src: string, mimeType?: string, sizes?: string[], theme?: \"light\" | \"dark\"}[]"));
    }
}
