use mcpl::types::{Type, derive_schema};
use serde_json::{Value, json};

#[test]
fn enum_intersections_keep_representable_alternatives() {
    let result = derive_schema(&json!({"type":["string","null"], "enum":["yes",null]}));
    assert_eq!(result.ty.to_string(), "\"yes\" | null");
    assert!(result.notes.is_empty());
    let result = derive_schema(&json!({"allOf":[{"enum":[1,2]},{"enum":[2,3]}]}));
    assert_eq!(result.ty.to_string(), "2");
    assert!(result.notes.is_empty());
}

#[test]
fn deep_and_wide_schemas_degrade_without_panicking() {
    let mut deep = json!({"type":"string"});
    for _ in 0..140 {
        deep = json!({"type":"array", "items":deep});
    }
    for schema in [deep, json!({"enum":vec![Value::Null; 20_001]})] {
        let result = derive_schema(&schema);
        assert_eq!(result.ty, Type::Any);
        assert!(result.notes.iter().any(|n| n.message.contains("budget")));
    }
}

#[test]
fn repeated_acyclic_references_have_an_expansion_budget() {
    let mut definitions = serde_json::Map::new();
    definitions.insert("Level0".into(), json!({"type":"string"}));
    for i in 1..25 {
        let reference = json!({"$ref":format!("#/$defs/Level{}", i - 1)});
        definitions.insert(format!("Level{i}"), json!({"type":"object", "properties":{"left":reference,"right":reference}, "additionalProperties":false}));
    }
    let result = derive_schema(&json!({"$defs": definitions, "$ref":"#/$defs/Level24"}));
    assert!(result.notes.iter().any(|n| n.message.contains("budget")));
    assert!(result.ty.to_string().len() < 1_000_000);
}

#[test]
fn illustrative_spec_server_renders_verbatim() {
    use mcpl::registry::ToolDefinition;
    use mcpl::types::derive_server;
    let tools = [
        ("browser_navigate", json!({"type":"object", "properties":{"url":{"type":"string"}}, "required":["url"], "additionalProperties":false})),
        ("browser_snapshot", json!({"type":"object", "additionalProperties":false})),
    ].map(|(name, input_schema)| ToolDefinition {
        name: name.into(), description: None, input_schema, output_schema: None, annotations: None,
    });
    assert_eq!(
        derive_server("playwright", &tools).ty.to_string(),
        "{browser_navigate: (args: {url: string}) => Content[], browser_snapshot: (args: {}) => Content[]}"
    );
}
