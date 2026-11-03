use mcpl::registry::ToolDefinition;
use mcpl::types::{
    DerivedType, Type, content_result_type, content_type, derive_schema, derive_server,
    derive_tool, render_tool_signature,
};
use serde_json::{Value, json};

fn clean(schema: Value, expected: &str) -> DerivedType {
    let derived = derive_schema(&schema);
    assert_eq!(derived.ty.to_string(), expected, "schema: {schema}");
    assert!(
        derived.notes.is_empty(),
        "schema: {schema}; notes: {:?}",
        derived.notes
    );
    derived
}

fn noted(schema: Value, expected: &str, path: &str, message: &str) -> DerivedType {
    let derived = derive_schema(&schema);
    assert_eq!(derived.ty.to_string(), expected, "schema: {schema}");
    assert!(
        derived
            .notes
            .iter()
            .any(|note| note.path == path && note.message.contains(message)),
        "schema: {schema}; missing note at {path:?} containing {message:?}: {:?}",
        derived.notes
    );
    derived
}

#[test]
fn empty_boolean_and_untyped_schemas() {
    clean(json!(true), "any");
    clean(json!({}), "any");
    noted(json!(false), "any", "", "bottom");
    clean(json!({"description":"not a type"}), "any");
    clean(json!({"properties":{"x":{"type":"string"}}}), "any");
    clean(json!({"items":{"type":"string"}}), "any");
    clean(json!({"minimum":2}), "any");
}

#[test]
fn primitive_types_and_type_lists() {
    for (kind, expected) in [
        ("string", "string"),
        ("number", "number"),
        ("integer", "integer"),
        ("boolean", "boolean"),
        ("null", "null"),
        ("object", "{[key: string]: any}"),
        ("array", "any[]"),
    ] {
        clean(json!({"type":kind}), expected);
    }
    clean(json!({"type":["string","null","string"]}), "string | null");
    assert_eq!(
        derive_schema(&json!({"type":"integer"})).ty,
        Type::Number { integer: true }
    );
}

#[test]
fn scalar_enum_and_const_keep_literals() {
    clean(
        json!({"enum":["red",2,true,null,"red"]}),
        "\"red\" | 2 | true | null",
    );
    clean(
        json!({"type":"string","enum":["red","blue"]}),
        "\"red\" | \"blue\"",
    );
    for (value, expected) in [
        (json!("x"), "\"x\""),
        (json!(7), "7"),
        (json!(false), "false"),
        (json!(null), "null"),
    ] {
        clean(json!({"const":value}), expected);
    }
}

#[test]
fn composite_enum_and_const_have_closed_shapes() {
    clean(
        json!({"const":{"name":"x","values":[1,null]}}),
        "{name: \"x\", values: [1, null]}",
    );
    clean(
        json!({"enum":[{"kind":"a"},[true,"b"]]}),
        "{kind: \"a\"} | [true, \"b\"]",
    );
    clean(json!({"const":[]}), "[]");
    clean(json!({"const":{}}), "{}");
}

#[test]
fn object_fields_requiredness_and_additional_values_are_independent() {
    clean(
        json!({"type":"object","properties":{"z":{"type":"string"},"a":{"type":"number"}},"required":["z"],"additionalProperties":false}),
        "{z: string, a?: number}",
    );
    clean(
        json!({"type":"object","properties":{"x":{"type":"string"}},"additionalProperties":true}),
        "{x?: string, [key: string]: any}",
    );
    clean(
        json!({"type":"object","properties":{"name":{"type":"string"}},"required":["other"],"additionalProperties":{"type":"integer"}}),
        "{name?: string, other: integer, [key: string]: integer}",
    );
    noted(
        json!({"type":"object","required":["missing"],"additionalProperties":false}),
        "{missing: any}",
        "",
        "undeclared required",
    );
}

#[test]
fn pattern_properties_union_dynamic_values_but_not_named_fields() {
    noted(
        json!({"type":"object","properties":{"label":{"type":"boolean"}},"patternProperties":{"^x":{"type":"string"},"^y":{"type":"number"}},"additionalProperties":false}),
        "{label?: boolean, [key: string]: string | number}",
        "",
        "pattern keys",
    );
    noted(
        json!({"type":"object","patternProperties":{"^x":{"type":"string"}}}),
        "{[key: string]: any}",
        "",
        "pattern keys",
    );
    noted(
        json!({"type":"object","patternProperties":{"^x":{"type":"string"}},"additionalProperties":{"type":"boolean"}}),
        "{[key: string]: boolean | string}",
        "",
        "pattern keys",
    );
}

#[test]
fn cross_property_and_key_constraints_preserve_base_shape() {
    clean(
        json!({"type":"object","properties":{"x":{"type":"string"}},"propertyNames":{"pattern":"^[a-z]+$"},"dependentRequired":{"x":["y"]},"dependentSchemas":{"x":{"required":["y"]}},"additionalProperties":false}),
        "{x?: string}",
    );
}

#[test]
fn homogeneous_arrays_and_empty_only_arrays() {
    clean(
        json!({"type":"array","items":{"type":"string"}}),
        "string[]",
    );
    clean(json!({"type":"array","items":true}), "any[]");
    clean(json!({"type":"array","items":false}), "[]");
    clean(json!({"type":"array","additionalItems":false}), "any[]");
}

#[test]
fn tuple_positions_are_optional_unless_min_items_requires_them() {
    clean(
        json!({"type":"array","prefixItems":[{"type":"string"},{"type":"number"}]}),
        "[string?, number?, ...any[]]",
    );
    clean(
        json!({"type":"array","prefixItems":[{"type":"string"},{"type":"number"}],"minItems":1,"items":{"type":"boolean"}}),
        "[string, number?, ...boolean[]]",
    );
    clean(
        json!({"type":"array","prefixItems":[{"type":"string"},{"type":"number"}],"minItems":2,"items":false}),
        "[string, number]",
    );
    clean(
        json!({"type":"array","prefixItems":[{"type":["string","null"]}],"items":true}),
        "[(string | null)?, ...any[]]",
    );
}

#[test]
fn legacy_tuple_items_and_additional_items() {
    let draft = "http://json-schema.org/draft-07/schema#";
    clean(
        json!({"$schema":draft,"type":"array","items":[{"type":"string"},{"type":"integer"}],"additionalItems":false,"minItems":1}),
        "[string, integer?]",
    );
    clean(
        json!({"$schema":draft,"type":"array","items":[{"type":"string"}],"additionalItems":{"type":"number"}}),
        "[string?, ...number[]]",
    );
    clean(
        json!({"$schema":draft,"type":"array","items":{"type":"string"},"additionalItems":false}),
        "string[]",
    );
}

#[test]
fn array_validation_keywords_do_not_change_element_type() {
    clean(
        json!({"type":"array","items":{"type":"integer"},"contains":{"const":1},"minContains":1,"maxContains":2,"uniqueItems":true,"minItems":1,"maxItems":4}),
        "integer[]",
    );
}

#[test]
fn unions_flatten_and_deduplicate_in_discovery_order() {
    clean(
        json!({"anyOf":[{"type":"string"},{"oneOf":[{"type":"number"},{"type":"string"}]},{"type":"null"}]}),
        "string | number | null",
    );
    clean(
        json!({"oneOf":[{"type":"string"},{"type":"number"}]}),
        "string | number",
    );
    clean(json!({"anyOf":[{}, {"type":"string"}]}), "any");
}

#[test]
fn all_of_merges_compatible_records_and_numeric_constraints() {
    clean(
        json!({"allOf":[{"type":"object","properties":{"x":{"type":"number"}}},{"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"boolean"}},"required":["x"]}]}),
        "{x: integer, y?: boolean, [key: string]: any}",
    );
    clean(
        json!({"allOf":[{"type":"number"},{"type":"integer"}]}),
        "integer",
    );
    clean(
        json!({"allOf":[{"type":"array","items":{"type":"number"}},{"type":"array","items":{"type":"integer"}}]}),
        "integer[]",
    );
    noted(
        json!({"allOf":[{"type":"string"},{"type":"number"}]}),
        "any",
        "/allOf",
        "intersection",
    );
}

#[test]
fn negation_and_conditionals_have_explicit_approximation_notes() {
    noted(
        json!({"type":"string","not":{"const":"bad"}}),
        "string",
        "",
        "negation",
    );
    noted(json!({"not":{"type":"string"}}), "any", "", "negation");
    noted(
        json!({"if":{"type":"string"},"then":{"type":"string"},"else":{"type":"integer"}}),
        "string | integer",
        "",
        "conditional",
    );
    noted(
        json!({"type":"number","if":{"minimum":0},"then":{"type":"integer"}}),
        "number",
        "",
        "conditional",
    );
    clean(
        json!({"type":"boolean","then":{"type":"string"},"else":{"type":"number"}}),
        "boolean",
    );
}

#[test]
fn local_json_pointer_refs_handle_escaping_and_percent_encoding() {
    clean(
        json!({"$defs":{"a/b~c":{"type":"string"}},"$ref":"#/$defs/a~1b~0c"}),
        "string",
    );
    clean(
        json!({"$defs":{"space name":{"type":"integer"}},"$ref":"#/$defs/space%20name"}),
        "integer",
    );
    clean(
        json!({"definitions":{"Legacy":{"type":"boolean"}},"$ref":"#/definitions/Legacy"}),
        "boolean",
    );
    let unused = clean(
        json!({"type":"string","$defs":{"unused":{"type":"number"}}}),
        "string",
    );
    assert!(unused.definitions.is_empty());
}

#[test]
fn anchors_and_nested_resource_ids_resolve_without_external_loading() {
    clean(
        json!({"$id":"https://example.test/root","$defs":{"word":{"$anchor":"word","type":"string"}},"$ref":"#word"}),
        "string",
    );
    clean(
        json!({"$id":"https://example.test/root","$defs":{"child":{"$id":"child","$defs":{"value":{"type":"integer"}},"$ref":"#/$defs/value"}},"$ref":"child"}),
        "integer",
    );
    clean(
        json!({"$id":"https://example.test/root","$defs":{"child":{"$id":"child","$defs":{"value":{"$anchor":"value","type":"boolean"}}}},"$ref":"child#value"}),
        "boolean",
    );
}

#[test]
fn recursive_reference_is_named_and_definition_is_finite() {
    let derived = clean(
        json!({"$defs":{"Node":{"type":"object","properties":{"value":{"type":"string"},"children":{"type":"array","items":{"$ref":"#/$defs/Node"}}},"required":["value"],"additionalProperties":false}},"$ref":"#/$defs/Node"}),
        "{value: string, children?: Node[]}",
    );
    assert_eq!(derived.definitions.len(), 1);
    assert_eq!(
        derived.render_definitions(),
        "Node = {value: string, children?: Node[]}"
    );
    let root = clean(
        json!({"type":"object","properties":{"next":{"$ref":"#"}},"additionalProperties":false}),
        "{next?: Root}",
    );
    assert_eq!(root.render_definitions(), "Root = {next?: Root}");
}

#[test]
fn mutually_recursive_refs_are_finite_and_repeatable() {
    let schema = json!({"$defs":{"A":{"type":"object","properties":{"b":{"$ref":"#/$defs/B"}},"additionalProperties":false},"B":{"type":"object","properties":{"a":{"$ref":"#/$defs/A"}},"additionalProperties":false}},"$ref":"#/$defs/A"});
    let derived = clean(schema.clone(), "{b?: {a?: A}}");
    assert_eq!(derived.render_definitions(), "A = {b?: {a?: A}}");
    assert_eq!(derived, derive_schema(&schema));
}

#[test]
fn ref_siblings_follow_modern_and_legacy_dialects() {
    clean(
        json!({"$defs":{"T":{"type":"number"}},"$ref":"#/$defs/T","type":"integer"}),
        "integer",
    );
    for draft in ["draft-04", "draft-06", "draft-07"] {
        clean(
            json!({"$schema":format!("http://json-schema.org/{draft}/schema#"),"definitions":{"T":{"type":"number"}},"$ref":"#/definitions/T","type":"string"}),
            "number",
        );
    }
    clean(
        json!({"$schema":"http://json-schema.org/draft-04/schema#","id":"https://example.test/legacy","definitions":{"T":{"id":"#word","type":"string"}},"$ref":"#word"}),
        "string",
    );
}

#[test]
fn dynamic_and_recursive_ref_fallbacks_are_documented() {
    noted(
        json!({"$dynamicAnchor":"node","$dynamicRef":"#node"}),
        "any",
        "",
        "dynamic reference",
    );
    noted(
        json!({"$recursiveAnchor":true,"$recursiveRef":"#","type":"object","additionalProperties":false}),
        "{}",
        "",
        "dynamic reference",
    );
    clean(json!({"$dynamicAnchor":"node","type":"string"}), "string");
    clean(json!({"$recursiveAnchor":true,"type":"integer"}), "integer");
}

#[test]
fn nullable_is_an_extension_not_coercion() {
    clean(json!({"type":"string","nullable":true}), "string | null");
    clean(
        json!({"type":["string","null"],"nullable":true}),
        "string | null",
    );
    clean(json!({"type":"string","nullable":false}), "string");
    clean(json!({"nullable":true}), "any");
}

#[test]
fn validation_and_documentation_metadata_do_not_transform_values() {
    clean(
        json!({"type":"string","format":"date-time","pattern":"^x","minLength":1,"maxLength":10,"contentEncoding":"base64","contentMediaType":"application/json","contentSchema":{"type":"object"},"title":"Example","description":"description","default":42,"examples":[3],"$comment":"comment","readOnly":true,"writeOnly":true,"deprecated":true}),
        "string",
    );
    clean(
        json!({"type":"number","minimum":0,"maximum":10,"exclusiveMinimum":1,"exclusiveMaximum":9,"multipleOf":0.5}),
        "number",
    );
    clean(
        json!({"$schema":"http://json-schema.org/draft-04/schema#","type":"number","minimum":0,"maximum":10,"exclusiveMinimum":true,"exclusiveMaximum":true}),
        "number",
    );
    clean(
        json!({"type":"object","minProperties":1,"maxProperties":4,"additionalProperties":false}),
        "{}",
    );
    clean(
        json!({"$schema":"https://example.test/unknown-dialect","$vocabulary":{"https://example.test/custom":true},"type":"boolean"}),
        "boolean",
    );
}

#[test]
fn unevaluated_constraints_keep_sound_base_approximations_with_notes() {
    noted(
        json!({"type":"object","properties":{"x":{"type":"string"}},"unevaluatedProperties":false}),
        "{x?: string, [key: string]: any}",
        "",
        "unevaluatedProperties",
    );
    noted(
        json!({"type":"array","prefixItems":[{"type":"string"}],"unevaluatedItems":false}),
        "[string?, ...any[]]",
        "",
        "unevaluatedItems",
    );
}

#[test]
fn unsupported_information_is_local_and_never_panics() {
    noted(
        json!({"x-custom":{"type":"integer"}}),
        "any",
        "/x-custom",
        "unsupported keyword",
    );
    noted(
        json!({"type":"string","x-custom":true}),
        "string",
        "/x-custom",
        "unsupported keyword",
    );
    noted(
        json!({"type":"object","properties":{"bad":{"type":"alien"},"good":{"type":"string"}},"additionalProperties":false}),
        "{bad?: any, good?: string}",
        "/properties/bad",
        "unknown type",
    );
    noted(
        json!({"type":"object","properties":{"a/b~c":{"$ref":"https://example.invalid/no-fetch"}},"additionalProperties":false}),
        "{\"a/b~c\"?: any}",
        "/properties/a~1b~0c",
        "unresolved reference",
    );
}

#[test]
fn malformed_schemas_and_keyword_values_have_fallback_notes() {
    for schema in [json!(null), json!(7), json!("string"), json!([])] {
        noted(schema, "any", "", "malformed schema");
    }
    for schema in [
        json!({"type":7}),
        json!({"type":[]}),
        json!({"type":["string",7]}),
    ] {
        noted(schema, "any", "/type", "malformed type");
    }
    for schema in [json!({"enum":[]}), json!({"enum":false})] {
        noted(schema, "any", "/enum", "enum");
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        for value in [json!([]), json!(false)] {
            noted(
                json!({keyword:value}),
                "any",
                &format!("/{keyword}"),
                "applicator",
            );
        }
    }
    noted(
        json!({"type":"object","properties":[]}),
        "any",
        "",
        "malformed properties",
    );
    noted(
        json!({"type":"object","required":5}),
        "{[key: string]: any}",
        "",
        "malformed required",
    );
    noted(
        json!({"type":"object","patternProperties":false,"additionalProperties":false}),
        "{[key: string]: any}",
        "",
        "malformed patternProperties",
    );
    noted(
        json!({"type":"array","prefixItems":{}}),
        "any",
        "",
        "malformed tuple",
    );
    noted(
        json!({"type":"array","items":42}),
        "any[]",
        "/items",
        "malformed schema",
    );
    noted(json!({"$ref":42}), "any", "", "malformed $ref");
    for reference in [
        "#/missing",
        "#/%ZZ",
        "https://example.invalid/schema",
        "file:///no-local-files.json",
    ] {
        noted(json!({"$ref":reference}), "any", "", "unresolved reference");
    }
}

fn tool(name: &str, input: Value, output: Option<Value>) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: None,
        input_schema: input,
        output_schema: output,
        annotations: None,
    }
}

#[test]
fn tool_identity_output_schema_and_content_default_are_distinct() {
    let input = json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false});
    let derived = derive_tool("playwright", &tool("browser_navigate", input.clone(), None));
    let Type::Tool(capability) = &derived.ty else {
        panic!("expected tool")
    };
    assert_eq!(capability.server, "playwright");
    assert_eq!(capability.name, "browser_navigate");
    assert_eq!(
        render_tool_signature(capability),
        "playwright.browser_navigate: (args: {url: string}) => Content[]"
    );
    assert_eq!(*capability.signature.result, content_result_type());
    assert_eq!(derived.definitions.get("Content"), Some(&content_type()));
    assert!(derived.notes.is_empty());
    let output = derive_tool("s", &tool("typed", input, Some(json!({"type":"integer"}))));
    assert_eq!(output.ty.to_string(), "(args: {url: string}) => integer");
    assert!(!output.definitions.contains_key("Content"));
    let explicit_any = derive_tool("s", &tool("any", json!({}), Some(json!(true))));
    assert_eq!(explicit_any.ty.to_string(), "(args: any) => any");
    assert!(explicit_any.definitions.is_empty());
}

fn recursive_schema(key: &str) -> Value {
    json!({"$defs":{key:{"type":"object","properties":{"next":{"$ref":format!("#/$defs/{key}")}},"additionalProperties":false}},"$ref":format!("#/$defs/{key}")})
}

#[test]
fn input_output_and_server_definition_names_do_not_collide() {
    let first = tool(
        "first",
        recursive_schema("Node"),
        Some(recursive_schema("Node")),
    );
    let second = tool("second", recursive_schema("Node"), None);
    let derived = derive_server("s", [&first, &second]);
    assert!(derived.notes.is_empty());
    assert_eq!(
        derived.ty.to_string(),
        "{first: (args: {next?: Node}) => {next?: Node_2}, second: (args: {next?: Node_3}) => Content[]}"
    );
    assert_eq!(derived.definitions["Node"].to_string(), "{next?: Node}");
    assert_eq!(derived.definitions["Node_2"].to_string(), "{next?: Node_2}");
    assert_eq!(derived.definitions["Node_3"].to_string(), "{next?: Node_3}");
    assert_eq!(derived.definitions.len(), 4);
    let reserved = derive_tool("s", &tool("content", recursive_schema("Content"), None));
    assert_eq!(reserved.definitions["Content"], content_type());
    assert_eq!(
        reserved.definitions["SchemaContent"].to_string(),
        "{next?: SchemaContent}"
    );
}

#[test]
fn server_order_names_and_note_paths_follow_discovery() {
    let a = tool("z/~", json!({"type":"alien"}), Some(json!(false)));
    let b = tool("a.b", json!({}), None);
    let derived = derive_server("server", [&a, &b]);
    let Type::Server(fields) = &derived.ty else {
        panic!("expected server")
    };
    assert_eq!(
        fields
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["z/~", "a.b"]
    );
    for (_, ty) in fields {
        let Type::Tool(tool) = ty else {
            panic!("expected tool")
        };
        assert_eq!(tool.server, "server");
    }
    assert!(
        derived
            .notes
            .iter()
            .any(|n| n.path == "/tools/z~1~0/inputSchema")
    );
    assert!(
        derived
            .notes
            .iter()
            .any(|n| n.path == "/tools/z~1~0/outputSchema")
    );
    assert_eq!(derive_server("empty", []).ty, Type::Server(vec![]));
}

fn fixture_tools() -> Vec<ToolDefinition> {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/playwright-tools.json")).unwrap();
    fixture["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|raw| ToolDefinition {
            name: raw["name"].as_str().unwrap().into(),
            description: raw
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            input_schema: raw["inputSchema"].clone(),
            output_schema: raw.get("outputSchema").cloned(),
            annotations: raw.get("annotations").cloned(),
        })
        .collect()
}

fn assert_no_any(ty: &Type) {
    match ty {
        Type::Any => panic!("unexpected any in fixture type"),
        Type::Array(item) => assert_no_any(item),
        Type::Tuple(tuple) => {
            for item in &tuple.items {
                assert_no_any(&item.ty);
            }
            if let Some(rest) = &tuple.rest {
                assert_no_any(rest);
            }
        }
        Type::Record(record) => {
            for field in &record.fields {
                assert_no_any(&field.ty);
            }
            if let Some(additional) = &record.additional {
                assert_no_any(additional);
            }
        }
        Type::Union(alternatives) => {
            for ty in alternatives {
                assert_no_any(ty);
            }
        }
        _ => {}
    }
}

fn assert_field_order(schema: &Value, ty: &Type) {
    match ty {
        Type::Record(record) => {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                assert_eq!(
                    record.fields.iter().map(|f| &f.name).collect::<Vec<_>>(),
                    properties.keys().collect::<Vec<_>>()
                );
                for field in &record.fields {
                    assert_field_order(&properties[&field.name], &field.ty);
                    let required = schema
                        .get("required")
                        .and_then(Value::as_array)
                        .is_some_and(|names| names.contains(&json!(field.name)));
                    assert_eq!(field.optional, !required, "{}", field.name);
                }
            }
        }
        Type::Array(item) => assert_field_order(&schema["items"], item),
        _ => {}
    }
}

#[test]
fn every_real_playwright_tool_derives_without_fallback_and_preserves_field_order() {
    let tools = fixture_tools();
    assert_eq!(tools.len(), 25);
    for tool in &tools {
        assert!(tool.output_schema.is_none());
        let derived = derive_tool("playwright", tool);
        assert!(
            derived.notes.is_empty(),
            "{}: {:?}",
            tool.name,
            derived.notes
        );
        let Type::Tool(capability) = &derived.ty else {
            panic!("expected tool")
        };
        assert_eq!(capability.server, "playwright");
        assert_eq!(capability.name, tool.name);
        assert_eq!(capability.signature.parameters.len(), 1);
        assert_eq!(capability.signature.parameters[0].name, "args");
        assert_eq!(*capability.signature.result, content_result_type());
        assert_no_any(&capability.signature.parameters[0].ty);
        assert_field_order(&tool.input_schema, &capability.signature.parameters[0].ty);
        assert_eq!(derived.definitions.len(), 1);
        assert_eq!(derived.definitions["Content"], content_type());
        assert_eq!(derived, derive_tool("playwright", tool));
    }
    let server = derive_server("playwright", &tools);
    assert!(server.notes.is_empty());
    assert_eq!(server.definitions.len(), 1);
    let Type::Server(fields) = &server.ty else {
        panic!("expected server")
    };
    assert_eq!(
        fields.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    for ((_, ty), tool) in fields.iter().zip(&tools) {
        assert_eq!(*ty, derive_tool("playwright", tool).ty);
    }
}

#[test]
fn real_navigate_and_snapshot_signatures_are_exact() {
    let tools = fixture_tools();
    for (name, expected) in [
        (
            "browser_navigate",
            "playwright.browser_navigate: (args: {url: string}) => Content[]",
        ),
        (
            "browser_snapshot",
            "playwright.browser_snapshot: (args: {target?: string, filename?: string, depth?: number, boxes?: boolean}) => Content[]",
        ),
    ] {
        let tool = tools.iter().find(|t| t.name == name).unwrap();
        let derived = derive_tool("playwright", tool);
        let Type::Tool(capability) = &derived.ty else {
            panic!("expected tool")
        };
        assert_eq!(render_tool_signature(capability), expected);
    }
}

#[test]
fn recursive_definition_names_disambiguate_sanitized_schema_keys() {
    let schema = json!({
        "$defs": {
            "a-b": {"type":"object","properties":{"next":{"$ref":"#/$defs/a-b"}},"additionalProperties":false},
            "a_b": {"type":"object","properties":{"next":{"$ref":"#/$defs/a_b"}},"additionalProperties":false}
        },
        "type":"object",
        "properties":{"left":{"$ref":"#/$defs/a-b"},"right":{"$ref":"#/$defs/a_b"}},
        "additionalProperties":false
    });
    let derived = clean(
        schema.clone(),
        "{left?: {next?: a_b}, right?: {next?: a_b_2}}",
    );
    assert_eq!(
        derived.render_definitions(),
        "a_b = {next?: a_b}\na_b_2 = {next?: a_b_2}"
    );
    assert_eq!(derived, derive_schema(&schema));
}

#[test]
fn discovery_renaming_reserves_names_already_used_by_the_imported_schema() {
    let first = tool("first", recursive_schema("Node"), None);
    let second = tool(
        "second",
        json!({
            "$defs": {
                "Node": {"type":"object","properties":{"next":{"$ref":"#/$defs/Node"}},"additionalProperties":false},
                "Node_2": {"type":"object","properties":{"next":{"$ref":"#/$defs/Node_2"}},"additionalProperties":false}
            },
            "type":"object",
            "properties":{"left":{"$ref":"#/$defs/Node"},"right":{"$ref":"#/$defs/Node_2"}},
            "additionalProperties":false
        }),
        None,
    );
    let derived = derive_server("s", [&first, &second]);
    assert!(derived.notes.is_empty());
    assert_eq!(
        derived.ty.to_string(),
        "{first: (args: {next?: Node}) => Content[], second: (args: {left?: {next?: Node_3}, right?: {next?: Node_2}}) => Content[]}"
    );
    assert_eq!(derived.definitions["Node_3"].to_string(), "{next?: Node_3}");
    assert_eq!(derived.definitions["Node_2"].to_string(), "{next?: Node_2}");
    assert_eq!(derived.definitions["Node"].to_string(), "{next?: Node}");
    assert_eq!(derived.definitions.len(), 4);
}

#[test]
fn older_definitions_preserve_recursive_names_and_dependency_annotations() {
    let derived = clean(
        json!({
            "$schema":"http://json-schema.org/draft-07/schema#",
            "definitions":{"Node":{
                "type":"object",
                "properties":{"next":{"$ref":"#/definitions/Node"}},
                "dependencies":{"next":["value"]},
                "additionalProperties":false
            }},
            "$ref":"#/definitions/Node"
        }),
        "{next?: Node}",
    );
    assert_eq!(derived.render_definitions(), "Node = {next?: Node}");
}

#[test]
fn excessive_schema_depth_is_bounded_and_reported() {
    let mut schema = json!({"type":"string"});
    for _ in 0..140 {
        schema = json!({"type":"array","items":schema});
    }
    let derived = derive_schema(&schema);
    assert!(
        derived
            .notes
            .iter()
            .any(|note| note.message.contains("budget exceeded"))
    );
    assert!(derived.ty.to_string().starts_with("any"));
}
