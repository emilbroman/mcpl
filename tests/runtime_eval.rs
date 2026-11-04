use mcpl::{
    frontend::{Category, Diagnostic, check_expression, check_program},
    registry::Registry,
    runtime::{Budget, Code, Evaluator, Value},
    session::SessionState,
};
use serde_json::{Value as Json, json};
use std::sync::Arc;

async fn run(source: &str, session: &mut SessionState) -> Result<Vec<Value>, Diagnostic> {
    let checked =
        check_program(source, &session.environment).unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    let code = Arc::new(
        Code::new(source.into(), checked.expressions)
            .with_definitions(session.environment.definitions.clone()),
    );
    let registry = Registry::default();
    let mut eval = Evaluator::new(&registry, session);
    let mut values = Vec::new();
    for statement in checked.statements {
        let value = eval
            .eval(&statement.statement.expression, code.clone())
            .await?;
        eval.store_mut().commit(&statement, value.clone())?;
        values.push(value);
    }
    Ok(values)
}
async fn expression(source: &str, session: &mut SessionState) -> Result<Value, Diagnostic> {
    let checked = check_expression(source, &session.environment)
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    let code = Arc::new(
        Code::new(source.into(), checked.expressions)
            .with_definitions(session.environment.definitions.clone()),
    );
    let registry = Registry::default();
    Evaluator::new(&registry, session)
        .eval(&checked.expression, code)
        .await
}
fn json_value(value: &Value) -> Json {
    value
        .to_json(&mut Budget::default(), Default::default(), false)
        .unwrap()
}

#[tokio::test]
async fn arithmetic_short_circuit_and_sequences() {
    let mut session = SessionState::default();
    let values=run("a = 2 + 3 * 4; false && (1 / 0 == 1); true || (1 / 0 == 1); true ? 1 : 1 / 0; (x = 3; y = x + 2; [x, y]);",&mut session).await.unwrap();
    assert_eq!(
        values.iter().map(json_value).collect::<Vec<_>>(),
        vec![
            json!(14),
            json!(false),
            json!(true),
            json!(1),
            json!([3, 5])
        ]
    );
}
#[tokio::test]
async fn all_callback_builtins_are_sequential_and_strict() {
    let mut session = SessionState::default();
    let source = r#"map([1, 2, 3], (x) => x * 2); filter([1, 2, 3], (x) => x > 1); reduce([1, 2, 3], (acc, x) => acc + x, 10); flat_map([1, 2], (x) => [x, x]); find([1, 2, 3], (x) => x > 1); any([1, 2], (x) => x > 1); all([1, 2], (x) => x > 0); sort_by([{n: 2, i: 0}, {n: 1, i: 1}, {n: 2, i: 2}], (x) => x.n); group_by(["a", "bb", "c"], (x) => str(len(x)));"#;
    let values = run(source, &mut session).await.unwrap();
    let expected = vec![
        json!([2, 4, 6]),
        json!([2, 3]),
        json!(16),
        json!([1, 1, 2, 2]),
        json!(2),
        json!(true),
        json!(true),
        json!([{"n":1,"i":1},{"n":2,"i":0},{"n":2,"i":2}]),
        json!({"1":["a","c"],"2":["bb"]}),
    ];
    assert_eq!(values.iter().map(json_value).collect::<Vec<_>>(), expected);
    assert_eq!(
        json_value(
            &expression("find([], (x) => true)", &mut session)
                .await
                .unwrap()
        ),
        json!(null)
    );
    assert_eq!(
        json_value(
            &expression("all([], (x) => false)", &mut session)
                .await
                .unwrap()
        ),
        json!(true)
    );
}
#[tokio::test]
async fn closures_capture_globals_and_lexical_values_across_runs() {
    let mut session = SessionState::default();
    run("a = 7; f = (x) => x + a; makers = map([1, 2], (x) => (y) => x + y); local = (a = 9; (x) => x + a);",&mut session).await.unwrap();
    assert_eq!(
        json_value(
            &expression(
                "[f(3), makers[0](10), makers[1](20), local(1)]",
                &mut session
            )
            .await
            .unwrap()
        ),
        json!([10, 11, 22, 10])
    );
}
#[tokio::test]
async fn checked_longest_match_and_literal_index_remain_distinct() {
    let mut session = SessionState::default();
    run("a = {b: {c: 1}}; a.b = {c: 2}; obj = {x: {y: 3}, \"x.y\": 4}; gradual = parse_json(to_json(obj));",&mut session).await.unwrap();
    assert_eq!(
        json_value(
            &expression(
                "[a.b.c, a[\"b\"].c, obj.x.y, obj[\"x\"].y, gradual.x.y]",
                &mut session
            )
            .await
            .unwrap()
        ),
        json!([2, 1, 4, 3, 3])
    );
}
#[tokio::test]
async fn runtime_failure_keeps_completed_bindings() {
    let mut session = SessionState::default();
    let error = run(
        "good = [10, 20]; total = sum(good); good[9]; unreachable = 0;",
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, Category::Runtime);
    assert!(error.message.contains("out of range"));
    assert_eq!(
        json_value(&expression("total", &mut session).await.unwrap()),
        json!(30)
    );
    assert!(!session.environment.bindings.contains_key("unreachable"));
}
#[tokio::test]
async fn self_application_exhausts_fuel_without_host_recursion() {
    let mut session = SessionState::default();
    run("f = (x) => x(x);", &mut session).await.unwrap();
    let error = expression("f(f)", &mut session).await.unwrap_err();
    assert_eq!(error.category, Category::Limit);
    assert!(error.message.contains("evaluation budget exhausted"));
    assert_eq!(error.span.start, 0);
}
#[tokio::test]
async fn fuel_and_growth_limits_are_checked() {
    let mut session = SessionState::default();
    for source in [
        "range(100001)",
        "map(range(100000), (x) => sum(range(100000)))",
    ] {
        assert_eq!(
            expression(source, &mut session).await.unwrap_err().category,
            Category::Limit
        );
    }
    assert!(
        expression("(f = parse_json(\"1\"); f(2))", &mut session)
            .await
            .unwrap_err()
            .message
            .contains("not callable")
    );
}
#[tokio::test]
async fn gradual_builtin_callbacks_enforce_results() {
    let mut session = SessionState::default();
    assert!(
        expression("filter([1], (x) => parse_json(\"1\"))", &mut session)
            .await
            .unwrap_err()
            .message
            .contains("boolean")
    );
    assert!(
        expression("flat_map([1], (x) => parse_json(\"1\"))", &mut session)
            .await
            .unwrap_err()
            .message
            .contains("array")
    );
    assert!(
        expression(
            "sort_by(parse_json(\"[1, \\\"a\\\"]\"), (x) => x)",
            &mut session
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn old_closure_diagnostics_reference_current_call_site() {
    let mut session = SessionState::default();
    run("f = (x) => x[999];", &mut session).await.unwrap();
    let error = expression("f([1])", &mut session).await.unwrap_err();
    assert_eq!(error.span, mcpl::frontend::Span::new(0, 6));
    assert!(error.message.contains("stored closure"));
}

#[tokio::test]
async fn retained_sharing_and_nested_growth_are_bounded() {
    let mut session = SessionState::default();
    let values = run("x = range(100); a = [x, x];", &mut session)
        .await
        .unwrap();
    assert!(
        Value::retained_bytes(&values) < values.iter().map(Value::logical_bytes).sum::<usize>()
    );
    let error = expression(
        "reduce(range(80), (acc, x) => [acc], parse_json(\"null\"))",
        &mut session,
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, Category::Limit);
    assert!(error.message.contains("depth"));
    let error = expression("(f = (x) => 1 + x(x); f(f))", &mut session)
        .await
        .unwrap_err();
    assert_eq!(error.category, Category::Limit);
}

#[tokio::test]
async fn tool_arguments_serialize_integral_numbers_without_fraction() {
    // Tool arguments go through Value::to_json; servers with `integer`
    // parameters (e.g. git_log max_count) reject `5.0`.
    let mut session = SessionState::default();
    let value = expression("{max_count: 5, nested: [-3, 2.5, 1e3]}", &mut session)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&json_value(&value)).unwrap(),
        r#"{"max_count":5,"nested":[-3,2.5,1000]}"#
    );
}

#[tokio::test]
async fn compact_json_normalizes_integral_numbers_and_equality_ignores_key_order() {
    let mut session = SessionState::default();
    assert_eq!(
        json_value(
            &expression("to_json([1, -0, 1.5])", &mut session)
                .await
                .unwrap()
        ),
        json!("[1,0,1.5]")
    );
    assert_eq!(json_value(&expression("parse_json(\"{\\\"x\\\": 1, \\\"y\\\": 2}\") == parse_json(\"{\\\"y\\\": 2, \\\"x\\\": 1}\")",&mut session).await.unwrap()),json!(true));
    assert_eq!(
        json_value(
            &expression("type_of(entries({a: 1}))", &mut session)
                .await
                .unwrap()
        ),
        json!("[string, number][]")
    );
}

#[tokio::test]
async fn empty_collections_still_check_gradual_callback_capabilities() {
    let mut session = SessionState::default();
    let error = expression("map([], parse_json(\"1\"))", &mut session)
        .await
        .unwrap_err();
    assert!(error.message.contains("callback"));
}
