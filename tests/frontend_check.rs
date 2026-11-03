use mcpl::frontend::checker::Root;
use mcpl::frontend::{Environment, check_expression, check_program, render_diagnostics};
use mcpl::types::{Type, derive_schema};
use serde_json::json;

fn succeeds(source: &str, environment: &Environment) -> mcpl::frontend::CheckedProgram {
    check_program(source, environment)
        .unwrap_or_else(|errors| panic!("{source}\n{}", render_diagnostics(&errors, source)))
}
fn fails(source: &str, environment: &Environment, message: &str) {
    let errors = check_program(source, environment).expect_err(source);
    let rendered = render_diagnostics(&errors, source);
    assert!(
        rendered.contains(message),
        "{source}\n{rendered}\nexpected {message}"
    );
}
fn typed(source: &str, expected: &str) {
    let checked = succeeds(source, &Environment::default());
    assert_eq!(
        checked
            .statements
            .last()
            .unwrap()
            .binding
            .display_type
            .to_string(),
        expected,
        "{source}"
    );
}
fn bind_schema(env: &mut Environment, name: &str, schema: serde_json::Value) {
    let derived = derive_schema(&schema);
    env.definitions.extend(derived.definitions);
    env.bind(name, derived.ty);
}

#[test]
fn all_builtin_families_infer_and_reject_bad_arguments() {
    for (source, expected) in [
        ("len([1]);", "number"),
        ("len({x: 1});", "number"),
        ("map([1, 2], (x) => str(x));", "string[]"),
        ("filter([1, 2], (x) => x > 1);", "number[]"),
        ("reduce([1, 2], (acc, x) => acc + x, 0);", "number"),
        ("flat_map([1], (x) => [x, x]);", "number[]"),
        ("find([1], (x) => true);", "number | null"),
        ("any([1], (x) => true);", "boolean"),
        ("all([1], (x) => false);", "boolean"),
        ("sort_by([1], (x) => str(x));", "number[]"),
        ("reverse([1]);", "number[]"),
        ("slice([1], 0);", "number[]"),
        ("concat([1], [\"a\"]);", "(number | string)[]"),
        ("unique([1]);", "number[]"),
        ("group_by([1], (x) => str(x));", "{[key: string]: number[]}"),
        ("zip([1], [\"a\"]);", "[number, string][]"),
        ("range(1);", "number[]"),
        ("range(0, 1);", "number[]"),
        ("sum([]);", "number"),
        ("min([1]);", "number"),
        ("max([1]);", "number"),
        ("first([1]);", "number"),
        ("last([1]);", "number"),
        ("keys({x: 1});", "string[]"),
        ("values({x: 1, y: \"a\"});", "(number | string)[]"),
        ("entries({x: 1});", "[string, number][]"),
        ("from_entries([[\"a\", 1]]);", "{[key: string]: number}"),
        (
            "merge({x: 1}, {y: \"a\"});",
            "{[key: string]: number | string}",
        ),
        ("has({x: 1}, \"x\");", "boolean"),
        ("str(1);", "string"),
        ("num(\"1\");", "number"),
        ("split(\"a\", \",\");", "string[]"),
        ("join([\"a\"], \",\");", "string"),
        ("trim(\"a\");", "string"),
        ("lower(\"a\");", "string"),
        ("upper(\"a\");", "string"),
        ("contains(\"a\", \"b\");", "boolean"),
        ("contains([1], 2);", "boolean"),
        ("starts_with(\"a\", \"b\");", "boolean"),
        ("ends_with(\"a\", \"b\");", "boolean"),
        ("replace(\"a\", \"b\", \"c\");", "string"),
        ("substring(\"a\", 0);", "string"),
        ("index_of(\"a\", \"b\");", "number"),
        ("lines(\"a\");", "string[]"),
        ("regex_match(\"a\", r\"\\d\");", "boolean"),
        ("regex_find_all(\"a\", r\"\\d\");", "string[][]"),
        ("regex_replace(\"a\", r\"\\d\", \"b\");", "string"),
        ("floor(1);", "number"),
        ("ceil(1);", "number"),
        ("round(1);", "number"),
        ("abs(1);", "number"),
        ("parse_json(\"{}\");", "any"),
        ("to_json({x: 1});", "string"),
        ("text([{\"type\": \"text\", text: \"a\"}]);", "string"),
        ("type_of(1);", "string"),
    ] {
        typed(source, expected);
    }
    let env = Environment::default();
    for (source, message) in [
        (
            "map([1], (x) => upper(x));",
            "expected string, found number",
        ),
        ("filter([1], (x) => x);", "expected (x: number) => boolean"),
        (
            "reduce([1], (a, x) => str(a), 0);",
            "expected (acc: number, x: number) => number",
        ),
        ("flat_map([1], (x) => x);", "expected (x: number) => any[]"),
        (
            "sort_by([1], (x) => true);",
            "sort_by key must be number or string",
        ),
        ("keys([1]);", "expected {[key: string]: any}"),
        ("from_entries([[1, 2]]);", "expected [string, number][]"),
        ("contains([1], \"a\");", "expected number, found string"),
        ("len(1);", "len expects an array, string or record"),
        ("range();", "expected 1 to 2 arguments"),
        ("substring(\"a\");", "expected 2 to 3 arguments"),
        ("text([1]);", "expected Content[]"),
    ] {
        fails(source, &env, message);
    }
}

#[test]
fn lexical_scopes_longest_matching_and_resolution_metadata() {
    let env = Environment::default();
    let source =
        "a = {b: {c: 1}}; a.b = {c: \"outer\"}; (a = {b: {c: true}}; a.b.c); a.b.c; a[\"b\"].c;";
    let checked = succeeds(source, &env);
    let types: Vec<_> = checked
        .statements
        .iter()
        .map(|s| s.binding.display_type.to_string())
        .collect();
    assert_eq!(&types[2..], &["boolean", "string", "number"]);
    let index = source.rfind("a.b.c;").unwrap();
    let info = &checked.expressions[&mcpl::frontend::Span::new(index, index + 5)];
    let resolution = info.resolution.as_ref().unwrap();
    assert_eq!(resolution.root, Some(Root::Session("a.b".into())));
    assert_eq!(resolution.members[0].key, "c");
    typed("r = {x: {y: 1}, x.y: \"joined\"}; r.x.y;", "string");
    typed("r = {x: {y: 1}, x.y: \"joined\"}; r[\"x\"].y;", "number");
    typed("map([\"a\"], (text) => upper(text));", "string[]");
    typed("(map = (x) => x + 1; map(1));", "number");
    typed("x = 1; (x = x + 1; x);", "number");
    for (source, message) in [
        ("(x, x) => x;", "x is already defined in this scope"),
        ("(x = 1; x = 2; x);", "x is already defined in this scope"),
        ("[x = 1; x, x];", "unknown name 'x'"),
        ("{a: x = 1; x, b: x};", "unknown name 'x'"),
        ("concat(x = [1]; x, x);", "unknown name 'x'"),
        ("x = (y) => x(y);", "unknown name 'x'"),
        ("x = y; y = 1;", "unknown name 'y'"),
        ("x = 1; x = 2;", "x is already defined"),
        ("map = 1;", "map is already defined"),
        (
            "a = {b: {c: 1}}; a.b = 2; a.b.c;",
            "property 'c' does not exist on number",
        ),
    ] {
        fails(source, &env, message);
    }
}

#[test]
fn aliases_capabilities_gradual_calls_and_lambda_context() {
    typed("m = map; m([1], (x) => str(x));", "string[]");
    typed("o = {m: map}; o.m([1], (x) => str(x));", "string[]");
    typed("ms = [map]; ms[0]([1], (x) => str(x));", "string[]");
    typed("get = () => map; get()([1], (x) => str(x));", "string[]");
    typed("map([\"a\"], upper);", "string[]");
    typed("f = (x) => x(x); f(f);", "any");
    typed("x = parse_json(\"{}\"); x.a.b(1, 2);", "any");
    typed("f = (x) => x; f(1);", "any");
    typed("f = (x) => x;", "(x: any) => any");
    let env = Environment::default();
    fails("[map] == [map];", &env, "equality requires JSON values");
    fails(
        "unique([map]);",
        &env,
        "unique requires JSON array elements",
    );
    fails(
        "map([1], (x, y) => x);",
        &env,
        "expected (x: number) => number",
    );
    fails("xs = [1]; xs.map((x) => x);", &env, "call map(xs, f)");
}

#[test]
fn strict_operators_and_arity() {
    let env = Environment::default();
    typed("1 + 2 * 3;", "number");
    typed("\"a\" + \"b\";", "string");
    typed("1 == 2;", "boolean");
    typed("true ? 1 : \"a\";", "number | string");
    for (source, message) in [
        ("\"a\" + 1;", "requires two numbers or two strings"),
        ("1 == \"1\";", "cannot compare disjoint types"),
        ("!1;", "expected boolean, found number"),
        ("1 && true;", "expected boolean, found number"),
        ("true || 1;", "expected boolean, found number"),
        ("true < false;", "requires two numbers or two strings"),
        ("-\"x\";", "expected number, found string"),
        ("((x) => x)();", "expected 1 arguments, found 0"),
        ("1();", "number is not callable"),
        ("1e999;", "numeric literal is not finite"),
    ] {
        fails(source, &env, message);
    }
}

#[test]
fn tuples_optional_fields_indexes_and_discriminants() {
    let mut env = Environment::default();
    bind_schema(
        &mut env,
        "entry",
        json!({"type":"array","prefixItems":[{"type":"string"},{"type":"number"}],"minItems":2,"items":false}),
    );
    bind_schema(
        &mut env,
        "obj",
        json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":{"type":"number"}}),
    );
    env.bind("key", Type::String);
    for (source, expected) in [
        ("entry[0];", "string"),
        ("entry[1];", "number"),
        ("obj.name;", "string"),
        ("obj[\"count\"];", "number"),
        ("obj[key];", "string | number"),
    ] {
        assert_eq!(
            succeeds(source, &env).statements[0]
                .binding
                .display_type
                .to_string(),
            expected
        );
    }
    fails("entry[2];", &env, "tuple index 2 out of range");
    fails("entry[-1];", &env, "tuple index -1 out of range");
    fails("obj[1];", &env, "record index: expected string");
    bind_schema(
        &mut env,
        "profile",
        json!({"type":"object","properties":{"name":{"type":"string"}},"additionalProperties":false}),
    );
    bind_schema(
        &mut env,
        "result",
        json!({"oneOf":[
            {"type":"object","properties":{"kind":{"const":"ok"},"value":{"type":"number"}},"required":["kind","value"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"error"},"message":{"type":"string"}},"required":["kind","message"],"additionalProperties":false}
        ]}),
    );
    for source in [
        "profile.name != null ? upper(profile.name) : \"none\";",
        "null != profile.name ? upper(profile.name) : \"none\";",
        "profile.name == null ? \"none\" : upper(profile.name);",
        "profile[\"name\"] != null && starts_with(profile.name, \"A\");",
        "(profile.name != null) ? upper(profile.name) : \"none\";",
        "result.kind == \"ok\" ? result.value : result.message;",
        "result.kind == \"ok\" && result.value > 0;",
        "(result = result; result.kind == \"ok\" ? result.value : result.message);",
    ] {
        succeeds(source, &env);
    }
    for source in [
        "profile.name != null || starts_with(profile.name, \"A\");",
        "null == profile.name ? \"none\" : upper(profile.name);",
        "c = profile.name != null; c ? upper(profile.name) : \"none\";",
        "result.value;",
        "result.kind != \"error\" ? result.value : 0;",
        "profile[key] != null ? upper(profile[key]) : \"none\";",
    ] {
        assert!(check_program(source, &env).is_err(), "{source}");
    }
}

#[test]
fn preflight_names_counter_and_environment_are_immutable() {
    let mut env = Environment::default();
    env.bind("res1", Type::String);
    env.reserved.insert("gone".into());
    let checked = succeeds("1; x = 2; 3;", &env);
    assert_eq!(
        checked
            .statements
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["res2", "x", "res3"]
    );
    assert_eq!(
        checked
            .statements
            .iter()
            .map(|s| s.next_result)
            .collect::<Vec<_>>(),
        [3, 3, 4]
    );
    assert_eq!(env.bindings.len(), 1);
    assert_eq!(env.next_result, 1);
    fails("gone = 1;", &env, "gone is already defined");
    fails(
        "1; res2 = 3;",
        &env,
        "conflicts with an explicit declaration",
    );
    succeeds("res2 = 3; 1;", &env);
    let shown = check_expression("(x = 1; x + 1)", &env).unwrap();
    assert_eq!(shown.info.display_type.to_string(), "number");
    assert_eq!(env.next_result, 1);
}

#[test]
fn reports_independent_errors_in_order_capped_at_ten() {
    let env = Environment::default();
    let source = (0..20)
        .map(|i| format!("unknown{i};\n"))
        .collect::<String>();
    let errors = check_program(&source, &env).unwrap_err();
    assert_eq!(errors.len(), 10);
    assert!(errors.windows(2).all(|e| e[0].span.start < e[1].span.start));
}

#[test]
fn contextual_builtin_callbacks_and_source_function_result_widening() {
    typed("f = () => 1; f();", "number");
    typed("map([1], range);", "number[][]");
    typed("map([[1]], first);", "number[]");
    typed("map([[1]], len);", "number[]");
    typed("map([[1]], reverse);", "number[][]");
    fails(
        "map([1], len);",
        &Environment::default(),
        "len expects an array",
    );
    fails(
        "map([1], upper);",
        &Environment::default(),
        "expected string, found number",
    );
}

#[test]
fn literal_schema_containers_and_enum_arguments() {
    use mcpl::types::{FunctionType, Parameter};
    let mut env = Environment::default();
    for (name, value) in [
        ("const_obj", json!({"x": 1})),
        ("const_arr", json!(["x", 1])),
    ] {
        bind_schema(&mut env, name, json!({"const":value}));
    }
    succeeds("const_obj.x; const_arr[1]; len(const_obj);", &env);
    fails("const_arr[2];", &env, "tuple index 2 out of range");
    let expected = derive_schema(&json!({"const": {"x": 1}})).ty;
    env.bind(
        "takes_const",
        Type::Function(FunctionType {
            parameters: vec![Parameter {
                name: "x".into(),
                ty: expected,
            }],
            result: Box::new(Type::Boolean),
        }),
    );
    succeeds("takes_const({x: 1});", &env);
    fails("takes_const({x: 2});", &env, "expected 1, found number");
    let expected = derive_schema(&json!({"enum":["a", "b"]})).ty;
    env.bind(
        "takes_enum",
        Type::Function(FunctionType {
            parameters: vec![Parameter {
                name: "x".into(),
                ty: expected,
            }],
            result: Box::new(Type::Boolean),
        }),
    );
    succeeds("x = \"a\"; takes_enum(x);", &env);
    fails("takes_enum(\"c\");", &env, "expected \"a\" | \"b\"");
}

#[test]
fn indexed_path_narrowing_and_schema_discriminants_survive_map() {
    let mut env = Environment::default();
    bind_schema(
        &mut env,
        "results",
        json!({"type":"array", "items":{"oneOf":[
            {"type":"object","properties":{"kind":{"const":"ok"},"value":{"type":"number"}},"required":["kind","value"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"err"},"error":{"type":"string"}},"required":["kind","error"],"additionalProperties":false}
        ]}}),
    );
    succeeds(
        "results[0].kind == \"ok\" ? results[0].value : results[0].error;",
        &env,
    );
    let checked = succeeds("map(results, (x) => x.kind);", &env);
    assert_eq!(
        checked.statements[0].binding.display_type.to_string(),
        "(\"ok\" | \"err\")[]"
    );
}

#[test]
fn recursive_types_and_static_expansion_limits_are_bounded() {
    let mut env = Environment::default();
    env.definitions.insert(
        "Loop".into(),
        Type::Union(vec![Type::Named("Loop".into()), Type::String]),
    );
    env.bind("cycle", Type::Named("Loop".into()));
    for source in ["cycle.x;", "cycle[0];", "cycle[\"x\"];", "len(cycle);"] {
        assert!(check_program(source, &env).is_err(), "{source}");
    }
    let mut source = "x0 = 1;".to_owned();
    for i in 1..25 {
        source.push_str(&format!("x{i} = {{a: x{}, b: x{}}};", i - 1, i - 1));
    }
    fails(
        &source,
        &Environment::default(),
        "static type expansion limit exceeded",
    );
    let deep = format!("{}1{};", "[".repeat(254), "]".repeat(254));
    succeeds(&deep, &Environment::default());
}

#[test]
fn real_playwright_fixture_calls_type_check_without_a_live_proxy() {
    use mcpl::registry::ToolDefinition;
    use mcpl::types::derive_server;
    use serde_json::Value;
    fn sample(schema: &Value) -> Value {
        if let Some(value) = schema.get("const") {
            return value.clone();
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return values[0].clone();
        }
        if let Some(variants) = schema
            .get("anyOf")
            .or_else(|| schema.get("oneOf"))
            .and_then(Value::as_array)
        {
            return sample(&variants[0]);
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("object") => {
                let mut fields = serde_json::Map::new();
                for name in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let name = name.as_str().unwrap();
                    fields.insert(name.into(), sample(&schema["properties"][name]));
                }
                Value::Object(fields)
            }
            Some("array") => json!([]),
            Some("string") => json!("fixture"),
            Some("number" | "integer") => json!(1),
            Some("boolean") => json!(false),
            _ => Value::Null,
        }
    }
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/playwright-tools.json")).unwrap();
    let tools: Vec<_> = fixture["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| ToolDefinition {
            name: tool["name"].as_str().unwrap().into(),
            description: None,
            input_schema: tool["inputSchema"].clone(),
            output_schema: tool.get("outputSchema").cloned(),
            annotations: None,
        })
        .collect();
    let mut env = Environment::default();
    env.add_server("playwright", derive_server("playwright", &tools))
        .unwrap();
    for tool in tools {
        let source = format!(
            "playwright[{}]({});",
            json!(tool.name),
            sample(&tool.input_schema)
        );
        assert_eq!(
            succeeds(&source, &env).statements[0]
                .binding
                .display_type
                .to_string(),
            "Content[]"
        );
    }
}

#[test]
fn union_callables_require_every_signature_and_share_contextual_resolution() {
    use mcpl::types::{FunctionType, Parameter, union};
    fn unary(input: Type, output: Type) -> Type {
        Type::Function(FunctionType {
            parameters: vec![Parameter {
                name: "x".into(),
                ty: input,
            }],
            result: Box::new(output),
        })
    }
    let mut env = Environment::default();
    env.bind(
        "choose",
        union([
            unary(Type::String, Type::String),
            unary(Type::number(), Type::number()),
        ]),
    );
    fails("choose(1);", &env, "expected string, found number");
    env.bind(
        "chooser",
        union([
            unary(
                unary(
                    derive_schema(&json!({"const":{"a":{"b":1}}})).ty,
                    Type::number(),
                ),
                Type::number(),
            ),
            unary(
                unary(
                    derive_schema(&json!({"const":{"a.b":1}})).ty,
                    Type::number(),
                ),
                Type::number(),
            ),
        ]),
    );
    fails(
        "chooser((x) => x.a.b);",
        &env,
        "property 'a' does not exist",
    );
}

#[test]
fn nested_discriminants_use_already_narrowed_prefixes() {
    let mut env = Environment::default();
    bind_schema(
        &mut env,
        "r",
        json!({"oneOf":[
            {"type":"object","properties":{"kind":{"const":"ok"},"child":{"oneOf":[
                {"type":"object","properties":{"kind":{"const":"one"},"value":{"type":"number"}},"required":["kind","value"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"const":"two"},"error":{"type":"string"}},"required":["kind","error"],"additionalProperties":false}
            ]}},"required":["kind","child"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"err"}},"required":["kind"],"additionalProperties":false}
        ]}),
    );
    succeeds(
        "r.kind == \"ok\" ? (r.child.kind == \"one\" ? str(r.child.value) : r.child.error) : \"outer\";",
        &env,
    );
}
