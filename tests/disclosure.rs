use std::sync::Arc;

use mcpl::{
    disclosure::{DisclosureLimits, binding_line, show, size},
    frontend::Span,
    runtime::{
        limits::Budget,
        value::{Value, ValueKind},
    },
    types::{Field, FunctionType, Parameter, RecordType, ToolType, Type},
};
use serde_json::json;

fn value(json: serde_json::Value) -> Value {
    Value::from_json(&json, &mut Budget::default(), Span::default()).unwrap()
}

fn typed(kind: ValueKind, ty: Type) -> Value {
    Value::new(kind, ty, &mut Budget::default(), Span::default()).unwrap()
}

#[test]
fn preview_uses_exact_counts_and_scalar_sizes() {
    let numbers = value(json!([1, 2, 3, 4, 5, 6, 7]));
    assert_eq!(
        binding_line("xs", &Type::Array(Box::new(Type::number())), &numbers),
        "xs: number[] (7 items) = [1, 2, 3, 4, 5, …+2]"
    );
    assert_eq!(
        binding_line("one", &Type::Boolean, &Value::boolean(true)),
        "one: boolean (1 value) = true"
    );
    assert_eq!(size(&value(json!([true]))), "1 item");
    assert_eq!(size(&value(json!({"a": 1}))), "1 key");
    assert_eq!(size(&value(json!("🦀"))), "1 char");
    assert_eq!(size(&value(json!("🦀e\u{301}"))), "3 chars");
}

#[test]
fn preview_depth_counts_only_containers_and_keeps_empty_containers() {
    let nested = value(json!([[1, [2], [], {}, {"a": 3}], {"b": {"c": true}}]));
    assert_eq!(
        binding_line("nested", &Type::Any, &nested),
        "nested: any (2 items) = [[1, […], [], {}, {…}], {b: {…}}]"
    );
    for (json, expected) in [
        (json!([]), "[]"),
        (json!({}), "{}"),
        (json!([1]), "[…]"),
        (json!({"a": 1}), "{…}"),
        (json!(1), "1"),
    ] {
        assert_eq!(
            show(
                &value(json),
                DisclosureLimits {
                    depth: 0,
                    ..Default::default()
                }
            ),
            expected
        );
    }
}

#[test]
fn previews_use_eight_object_keys_and_show_uses_shared_item_limit() {
    let fields = (0..10)
        .map(|i| (format!("k{i}"), Value::boolean(true)))
        .collect();
    let object = typed(ValueKind::Object(fields), Type::Any);
    assert_eq!(
        binding_line("obj", &Type::Any, &object),
        "obj: any (10 keys) = {k0: true, k1: true, k2: true, k3: true, k4: true, k5: true, k6: true, k7: true, …+2 keys}"
    );
    assert_eq!(
        show(
            &object,
            DisclosureLimits {
                max_items: 1,
                ..Default::default()
            }
        ),
        "{k0: true, …+9 keys}"
    );
    assert_eq!(
        show(
            &object,
            DisclosureLimits {
                max_items: 0,
                ..Default::default()
            }
        ),
        "{…+10 keys}"
    );
    assert_eq!(
        show(
            &value(json!([1, 2])),
            DisclosureLimits {
                max_items: 0,
                ..Default::default()
            }
        ),
        "[…+2]"
    );
}

#[test]
fn strings_are_unicode_safe_json_escaped_and_keep_full_size() {
    let text = "🦀".repeat(130);
    let string = value(json!(text));
    assert_eq!(
        binding_line("s", &Type::String, &string),
        format!("s: string (130 chars) = \"{}…\"", "🦀".repeat(120))
    );
    assert_eq!(
        show(
            &string,
            DisclosureLimits {
                max_string: 2,
                ..Default::default()
            }
        ),
        "string (130 chars) = \"🦀🦀…\""
    );
    assert_eq!(
        show(
            &value(json!("🦀\n\"\\\u{0000}")),
            DisclosureLimits::default()
        ),
        "\"🦀\\n\\\"\\\\\\u0000\""
    );
    assert_eq!(
        show(
            &value(json!("")),
            DisclosureLimits {
                max_string: 0,
                ..Default::default()
            }
        ),
        "\"\""
    );
    assert_eq!(
        show(
            &value(json!("a")),
            DisclosureLimits {
                max_string: 0,
                ..Default::default()
            }
        ),
        "string (1 char) = \"…\""
    );
}

#[test]
fn unsafe_keys_are_quoted_without_changing_the_key() {
    let object =
        value(json!({"a.b": 1, "a.2b": 2, "true": 3, ".a": 4, "a..b": 5, "quote\"": 6, "é": 7}));
    assert_eq!(
        show(&object, DisclosureLimits::default()),
        "{a.b: 1, a.2b: 2, \"true\": 3, \".a\": 4, \"a..b\": 5, \"quote\\\"\": 6, \"é\": 7}"
    );
}

#[test]
fn byte_budget_truncates_children_with_exact_remaining_counts() {
    let large = value(json!("🦀".repeat(200)));
    let array = typed(ValueKind::Array(vec![large.clone(); 100]), Type::Any);
    let shown = show(
        &array,
        DisclosureLimits {
            max_output: 256,
            ..Default::default()
        },
    );
    assert!(shown.len() <= 256, "{shown}");
    assert!(shown.starts_with("[\"🦀"), "{shown}");
    assert!(shown.ends_with("…\", …+99]"), "{shown}");
    let object = typed(
        ValueKind::Object((0..100).map(|i| (format!("k{i}"), large.clone())).collect()),
        Type::Any,
    );
    let shown = show(
        &object,
        DisclosureLimits {
            max_output: 256,
            ..Default::default()
        },
    );
    assert!(shown.len() <= 256, "{shown}");
    assert!(shown.ends_with("…\", …+99 keys}"), "{shown}");
    let line = binding_line("long", &Type::Any, &array);
    assert!(line.chars().count() <= 600);
    assert!(line.len() <= 2400);
}

#[test]
fn huge_keys_are_omitted_not_partially_disclosed_as_different_keys() {
    let object = typed(
        ValueKind::Object(vec![
            ("🦀".repeat(100_000), Value::null()),
            ("ok".into(), Value::null()),
        ]),
        Type::Any,
    );
    assert_eq!(
        show(
            &object,
            DisclosureLimits {
                max_output: 256,
                ..Default::default()
            }
        ),
        "{…+2 keys}"
    );
}

#[test]
fn capabilities_only_disclose_types() {
    let signature = FunctionType {
        parameters: vec![Parameter {
            name: "args".into(),
            ty: Type::String,
        }],
        result: Box::new(Type::Boolean),
    };
    let ty = Type::Tool(ToolType {
        server: "private-server".into(),
        name: "private-tool".into(),
        signature,
    });
    let tool = typed(
        ValueKind::Tool {
            server: "private-server".into(),
            name: "private-tool".into(),
        },
        ty.clone(),
    );
    assert_eq!(
        show(
            &tool,
            DisclosureLimits {
                depth: 0,
                ..Default::default()
            }
        ),
        "(args: string) => boolean"
    );
    assert_eq!(
        binding_line("f", &ty, &tool),
        "f: (args: string) => boolean (function) = (args: string) => boolean"
    );
    let server_ty = Type::Server(vec![("tool".into(), ty)]);
    let server = typed(
        ValueKind::Server("private-server".into()),
        server_ty.clone(),
    );
    assert_eq!(size(&server), "1 tool");
    assert_eq!(
        show(&server, DisclosureLimits::default()),
        server_ty.to_string()
    );
}

#[test]
fn wide_types_and_repeated_values_are_bounded_without_full_expansion() {
    let ty = Type::Record(RecordType {
        fields: (0..10_000)
            .map(|i| Field {
                name: format!("field{i}"),
                ty: Type::String,
                optional: false,
            })
            .collect(),
        additional: None,
    });
    let mut nested = Value::null();
    for _ in 0..16 {
        // Use sharing to make the logical rendering much larger than the retained tree.
        let data = mcpl::runtime::value::ValueData {
            kind: ValueKind::Array(vec![nested.clone(); 5]),
            ty: Arc::new(Type::Any),
            depth: 1,
            capture_depth: 0,
            logical_bytes: 64,
            own_bytes: 64,
        };
        nested = Value(Arc::new(data));
    }
    let line = binding_line("huge", &ty, &nested);
    assert!(line.chars().count() <= 600);
    assert!(line.contains('…'));
    let shown = show(
        &nested,
        DisclosureLimits {
            max_output: 256,
            ..Default::default()
        },
    );
    assert!(shown.len() <= 256);
    assert_eq!(
        shown.chars().filter(|&ch| ch == '[').count(),
        shown.chars().filter(|&ch| ch == ']').count()
    );
}

#[test]
fn escaped_strings_and_large_type_names_respect_all_output_caps() {
    let text = value(json!("\u{0000}🦀".repeat(100_000)));
    for max_output in [0, 1, 2, 3, 4, 5, 32, 256, 1024, 65_536, 100_000] {
        let shown = show(
            &text,
            DisclosureLimits {
                max_output,
                ..Default::default()
            },
        );
        assert!(shown.len() <= max_output.min(65_536));
        if max_output >= 256 {
            assert!(shown.starts_with("string (200000 chars) = \""));
            assert!(shown.ends_with("…\""));
        }
    }
    let ty = Type::Record(RecordType {
        fields: vec![Field {
            name: "🦀".repeat(100_000),
            ty: Type::String,
            optional: false,
        }],
        additional: None,
    });
    let line = binding_line("huge", &ty, &Value::null());
    assert!(line.chars().count() <= 600);
    assert!(line.ends_with(" (1 value) = null"));
}

#[test]
fn bounded_type_writer_matches_normal_type_syntax_when_untruncated() {
    use mcpl::types::{BuiltinType, TupleItem, TupleType};
    let union = Type::Union(vec![Type::String, Type::Null]);
    let tuple = Type::Tuple(TupleType {
        items: vec![TupleItem {
            ty: union.clone(),
            optional: true,
        }],
        rest: Some(Box::new(union.clone())),
    });
    let signature = FunctionType {
        parameters: vec![Parameter {
            name: "true".into(),
            ty: tuple.clone(),
        }],
        result: Box::new(Type::Array(Box::new(union.clone()))),
    };
    let function = Type::Function(signature.clone());
    let types = [
        Type::Array(Box::new(function.clone())),
        Type::Union(vec![function, Type::Null]),
        tuple,
        Type::Builtin(BuiltinType {
            name: "private".into(),
            signature,
        }),
        Type::Record(RecordType {
            fields: vec![Field {
                name: "with space".into(),
                ty: union,
                optional: true,
            }],
            additional: Some(Box::new(Type::Any)),
        }),
        Type::Literal(json!("\n🦀")),
        Type::Literal(json!(-0.0)),
    ];
    for ty in types {
        let capability = typed(ValueKind::Builtin("private".into()), ty.clone());
        assert_eq!(
            show(&capability, DisclosureLimits::default()),
            ty.to_string()
        );
    }
}
