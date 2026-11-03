//! Verbatim language-spec examples, checked without invoking a downstream server.
use mcpl::frontend::{
    Binding, Category, Diagnostic, Environment, Span, check_expression, check_program,
    render_diagnostics,
};
use mcpl::registry::ToolDefinition;
use mcpl::types::{Type, derive_schema, derive_server};
use serde_json::{Value, json};

macro_rules! source {
    ($name:literal) => {
        include_str!(concat!("fixtures/frontend/valid/", $name, ".mcpl"))
    };
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

fn environment() -> Environment {
    let mut env = Environment::default();
    let empty = json!({"type": "object", "additionalProperties": false});
    let playwright = [
        tool(
            "browser_navigate",
            json!({"type":"object", "properties":{"url":{"type":"string"}},
                "required":["url"], "additionalProperties":false}),
            None,
        ),
        tool("browser_snapshot", empty.clone(), None),
    ];
    env.add_server("playwright", derive_server("playwright", &playwright))
        .unwrap();
    let catalog = [
        tool(
            "profile",
            empty.clone(),
            Some(
                json!({"type":"object", "properties":{"name":{"type":"string"}},
                "additionalProperties":false}),
            ),
        ),
        tool(
            "lookup",
            empty.clone(),
            Some(json!({"oneOf":[
                {"type":"object", "properties":{"kind":{"const":"ok"}, "value":{"type":"number"}},
                 "required":["kind","value"], "additionalProperties":false},
                {"type":"object", "properties":{"kind":{"const":"error"}, "message":{"type":"string"}},
                 "required":["kind","message"], "additionalProperties":false}
            ]})),
        ),
    ];
    env.add_server("catalog", derive_server("catalog", &catalog))
        .unwrap();
    let tools = [
        tool("x", empty.clone(), None),
        tool("x.y", empty.clone(), None),
        tool("2fa-check", empty, None),
    ];
    env.add_server("s", derive_server("s", &tools)).unwrap();
    env
}

fn checked(source: &str, env: &Environment) -> mcpl::frontend::CheckedProgram {
    check_program(source, env)
        .unwrap_or_else(|errors| panic!("{}", render_diagnostics(&errors, source)))
}

fn listings(source: &str, env: &Environment) -> Vec<(String, String)> {
    checked(source, env)
        .statements
        .into_iter()
        .map(|statement| (statement.name, statement.binding.display_type.to_string()))
        .collect()
}

fn commit(source: &str, env: &mut Environment) -> Vec<(String, String)> {
    let program = checked(source, env);
    let mut result = Vec::new();
    for statement in program.statements {
        result.push((
            statement.name.clone(),
            statement.binding.display_type.to_string(),
        ));
        env.next_result = statement.next_result;
        env.reserved.insert(statement.name.clone());
        env.bindings.insert(statement.name, statement.binding);
    }
    result
}

fn expected(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(name, ty)| (name.to_string(), ty.to_string()))
        .collect()
}

#[test]
fn worked_a_discovers_first_class_server_and_tool() {
    assert_eq!(
        listings(source!("worked_a"), &environment()),
        expected(&[
            (
                "res1",
                "{browser_navigate: (args: {url: string}) => Content[], browser_snapshot: (args: {}) => Content[]}"
            ),
            ("res2", "(args: {url: string}) => Content[]"),
        ])
    );
}

#[test]
fn worked_b_preserves_bindings_across_runs_and_show_allocates_nothing() {
    for alternate in [false, true] {
        let mut env = environment();
        assert_eq!(
            commit(source!("worked_b_run1"), &mut env),
            expected(&[("res1", "Content[]"), ("res2", "string"),])
        );
        if alternate {
            commit(
                "regex_find_all(res2, r\"https://e[.]test/[a-z]\"); slice(res3, 0, 5);",
                &mut env,
            );
            assert_eq!(
                commit(source!("worked_b_alternate"), &mut env),
                expected(&[("res5", "{url: string, title: string}[]"),])
            );
        } else {
            assert_eq!(
                commit(source!("worked_b_run2"), &mut env),
                expected(&[
                    ("res3", "string[][]"),
                    ("res4", "string[][]"),
                    ("res5", "{url: string, title: string}[]"),
                ])
            );
        }
        assert_eq!(
            commit(source!("worked_b_run3"), &mut env),
            expected(&[("res6", "{url: string, title: string}"),])
        );
        let before = (env.bindings.len(), env.reserved.clone(), env.next_result);
        let show = check_expression("res5[1]", &env).unwrap();
        assert_eq!(
            show.info.display_type.to_string(),
            "{url: string, title: string}"
        );
        assert_eq!(
            (env.bindings.len(), env.reserved.clone(), env.next_result),
            before
        );
        assert_eq!(commit("res5[1];", &mut env)[0].0, "res7");
        assert!(!env.bindings.contains_key("snap"));
        assert!(!env.bindings.contains_key("link"));
    }
}

#[test]
fn worked_c_schema_optional_fields_and_discriminants() {
    assert_eq!(
        listings(source!("worked_c"), &environment()),
        expected(&[
            ("profile", "{name?: string}"),
            ("label", "string"),
            ("starts_a", "boolean"),
            (
                "result",
                "{kind: \"ok\", value: number} | {kind: \"error\", message: string}"
            ),
            ("description", "string"),
        ])
    );
}

#[test]
fn worked_d_runtime_index_failure_is_not_a_static_error() {
    assert_eq!(
        listings(source!("worked_d"), &environment()),
        expected(&[
            ("good", "number[]"),
            ("total", "number"),
            ("res1", "number"),
            ("unreachable", "number"),
        ])
    );
}

#[test]
fn worked_e_locals_do_not_become_session_bindings() {
    let mut env = environment();
    assert_eq!(
        commit(source!("worked_e"), &mut env),
        expected(&[("res1", "{match: string, count: number}"),])
    );
    assert!(!env.bindings.contains_key("source"));
    assert!(!env.bindings.contains_key("matches"));
    assert_eq!(env.next_result, 2);
}

#[test]
fn other_valid_fenced_programs_from_the_spec() {
    let mut operators = environment();
    for name in ["a", "b", "a-b"] {
        operators.bind(name, Type::number());
    }
    assert_eq!(
        listings(source!("operators"), &operators),
        expected(&[("res1", "number"), ("res2", "number"), ("res3", "boolean"),])
    );
    assert_eq!(
        listings(source!("longest_root"), &environment()),
        expected(&[
            ("a", "{b: {c: number}}"),
            ("a.b", "{c: number}"),
            ("res1", "number"),
            ("res2", "number"),
        ])
    );
    let members = checked(source!("server_members"), &environment());
    assert_eq!(members.statements.len(), 5);
    assert_eq!(
        members.statements[0].binding.display_type.to_string(),
        "(args: {}) => Content[]"
    );
    assert_eq!(
        members.statements[4].binding.display_type.to_string(),
        "Content[]"
    );
    for (source, rows) in [
        (
            source!("assignment"),
            expected(&[("answer", "number"), ("res1", "number")]),
        ),
        (
            source!("object_local"),
            expected(&[("res1", "{xyz: {abc: number}}")]),
        ),
        (
            source!("tool_alias"),
            expected(&[
                ("nav", "(args: {url: string}) => Content[]"),
                ("res1", "Content[]"),
            ]),
        ),
        (
            source!("contextual_lambda"),
            expected(&[("res1", "number[]")]),
        ),
        (
            source!("identity"),
            expected(&[("identity", "(x: any) => any")]),
        ),
        (
            source!("self_application"),
            expected(&[("f", "(x: any) => any"), ("res1", "any")]),
        ),
    ] {
        assert_eq!(listings(source, &environment()), rows, "{source}");
    }
    let mut narrowing = environment();
    narrowing.bind("user", derive_schema(&json!({"type":"object", "properties":{"name":{"type":"string"}}, "additionalProperties":false})).ty);
    let catalog = checked(source!("worked_c"), &narrowing);
    narrowing
        .bindings
        .insert("result".into(), catalog.statements[3].binding.clone());
    assert_eq!(
        listings(source!("narrowing"), &narrowing),
        expected(&[("res1", "string"), ("res2", "boolean"), ("res3", "string"),])
    );
}

#[test]
fn three_run_session_redefinition_is_rejected_without_mutating_environment() {
    let mut env = environment();
    assert_eq!(
        commit(source!("session_run1"), &mut env),
        expected(&[("res1", "{x: number}")])
    );
    assert_eq!(
        commit(source!("session_run2"), &mut env),
        expected(&[("res2", "{y: {x: number}}")])
    );
    let source = include_str!("fixtures/frontend/errors/redefinition.mcpl");
    let errors = check_program(source, &env).unwrap_err();
    assert_eq!(
        render_diagnostics(&errors, source),
        include_str!("fixtures/frontend/errors/redefinition.stderr").trim_end_matches('\n')
    );
    assert_eq!(env.next_result, 3);
    assert_eq!(env.bindings.len(), 2);
}

#[test]
fn section_13_frontend_diagnostics_match_verbatim() {
    let mut env = environment();
    env.bindings
        .insert("res2".into(), Binding::new(Type::number()));
    env.reserved.insert("res2".into());
    env.bind("record", derive_schema(&json!({"type":"object", "properties":{"x":{"type":"number"}}, "required":["x"], "additionalProperties":false})).ty);
    macro_rules! golden {
        ($name:literal) => {{
            let source = include_str!(concat!("fixtures/frontend/errors/", $name, ".mcpl"));
            let expected = include_str!(concat!("fixtures/frontend/errors/", $name, ".stderr"));
            let errors = check_program(source, &env).unwrap_err();
            assert_eq!(
                render_diagnostics(&errors, source),
                expected.trim_end_matches('\n'),
                "{}",
                $name
            );
        }};
    }
    golden!("terminator");
    golden!("operator_whitespace");
    golden!("sequence_brackets");
    golden!("unknown_name");
    golden!("redefinition");
    golden!("tool_argument");
    golden!("missing_property");
    golden!("condition");
}

#[test]
fn section_13_runtime_templates_render_without_runtime_execution() {
    macro_rules! golden {
        ($name:literal, $category:expr, $message:literal, $start:expr, $end:expr, $hint:literal) => {{
            let source = include_str!(concat!("fixtures/frontend/errors/", $name, ".mcpl"));
            let diagnostic =
                Diagnostic::new($category, $message, Span::new($start, $end)).with_hint($hint);
            let expected = include_str!(concat!("fixtures/frontend/errors/", $name, ".stderr"));
            assert_eq!(
                diagnostic.render(source),
                expected.trim_end_matches('\n'),
                "{}",
                $name
            );
        }};
    }
    golden!(
        "array_index",
        Category::Runtime,
        "array index 4 out of range for length 2",
        0,
        11,
        "valid indexes are 0 through 1"
    );
    golden!(
        "tool_failure",
        Category::Runtime,
        "playwright.browser_navigate: net::ERR_NAME_NOT_RESOLVED",
        0,
        61,
        "inspect the URL and retry the call as a separate program"
    );
    golden!(
        "evicted",
        Category::Runtime,
        "`res3` was evicted (session memory limit); recompute it",
        0,
        4,
        "rerun the computation under a new name"
    );
    let source = include_str!("fixtures/frontend/errors/fuel.mcpl");
    let start = source.find("map").unwrap();
    let diagnostic = Diagnostic::new(
        Category::Limit,
        "evaluation budget exhausted (10,000,000 units), statement 2",
        Span::new(start, start + 22),
    )
    .with_hint("reduce the data size or split the program");
    let expected = include_str!("fixtures/frontend/errors/fuel.stderr");
    // A previously committed binding is output by the runtime, not the diagnostic renderer.
    let (_, expected_diagnostic) = expected.split_once('\n').unwrap();
    assert_eq!(
        diagnostic.render(source),
        expected_diagnostic.trim_end_matches('\n')
    );
}
