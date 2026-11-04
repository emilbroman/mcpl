use mcpl::{
    frontend::{Diagnostic, check_expression, check_program},
    registry::Registry,
    runtime::{Budget, Code, Evaluator, Value},
    session::SessionState,
};
use serde_json::Value as Json;
use std::sync::Arc;

const PLAYWRIGHT_RESULT: &str = include_str!("fixtures/parse-json/playwright-result.txt");
const PLAYWRIGHT_PAYLOAD: &str = include_str!("fixtures/parse-json/playwright-payload.json");

async fn bind_fixture(name: &str, session: &mut SessionState) {
    let source = format!(
        "{name} = {};",
        serde_json::to_string(PLAYWRIGHT_RESULT).unwrap()
    );
    let checked = check_program(&source, &session.environment).unwrap();
    let code = Arc::new(
        Code::new(source.into(), checked.expressions)
            .with_definitions(session.environment.definitions.clone()),
    );
    let registry = Registry::default();
    let mut evaluator = Evaluator::new(&registry, session);
    for statement in checked.statements {
        let value = evaluator
            .eval(&statement.statement.expression, code.clone())
            .await
            .unwrap();
        evaluator.store_mut().commit(&statement, value).unwrap();
    }
}

async fn expression(source: &str, session: &mut SessionState) -> Result<Value, Diagnostic> {
    let checked = check_expression(source, &session.environment).unwrap();
    let code = Arc::new(
        Code::new(source.into(), checked.expressions)
            .with_definitions(session.environment.definitions.clone()),
    );
    let registry = Registry::default();
    Evaluator::new(&registry, session)
        .eval(&checked.expression, code)
        .await
}

fn assert_payload(value: Value) {
    let actual = value
        .to_json(&mut Budget::default(), Default::default(), false)
        .unwrap();
    let expected: Json = serde_json::from_str(PLAYWRIGHT_PAYLOAD).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn fixture_is_verbatim_playwright_text_from_committed_transcript() {
    let transcript = include_str!("../testing/transcripts/browser-demo/run-6.jsonl");
    let result = transcript
        .lines()
        .map(|line| serde_json::from_str::<Json>(line).unwrap())
        .find(|entry| {
            entry["type"] == "mcp_result"
                && entry["toolCallId"] == "call_d0e430f15bf940d880afedb64186e6f3"
        })
        .unwrap();
    let shown = result["result"]["content"][0]["text"].as_str().unwrap();
    let literal = shown
        .strip_prefix("[{type: \"text\", text: ")
        .unwrap()
        .strip_suffix("}]")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<String>(literal).unwrap(),
        PLAYWRIGHT_RESULT
    );
}

#[tokio::test]
async fn diagnostic_4_extraction_reports_trailing_playwright_code() {
    let mut session = SessionState::default();
    bind_fixture("body", &mut session).await;
    let source = r####"parse_json(split(split(body, "### Result\n")[1], "\n\n###")[0])"####;
    let error = expression(source, &mut session).await.unwrap_err();
    assert_eq!(
        error.render(source),
        include_str!("fixtures/parse-json/trailing-data.txt").trim_end()
    );

    assert_payload(
        expression(
            r####"parse_json(substring(split(split(body, "### Result\n")[1], "\n\n###")[0], 0, 229))"####,
            &mut session,
        )
        .await
        .unwrap(),
    );
}

#[tokio::test]
async fn diagnostic_8_extraction_parses_the_playwright_payload() {
    let mut session = SessionState::default();
    bind_fixture("raw", &mut session).await;
    assert_payload(
        expression(
            r####"parse_json(trim(split(split(raw, "### Result")[1], "###")[0]))"####,
            &mut session,
        )
        .await
        .unwrap(),
    );
}

#[tokio::test]
async fn whole_playwright_result_reports_leading_text_and_usable_bounds() {
    let mut session = SessionState::default();
    bind_fixture("raw", &mut session).await;
    let source = "parse_json(raw)";
    let error = expression(source, &mut session).await.unwrap_err();
    assert_eq!(
        error.render(source),
        include_str!("fixtures/parse-json/leading-data.txt").trim_end()
    );

    let suggestion = error.hint.unwrap();
    let suggested_expression = suggestion.split_once("; try ").unwrap().1;
    let source = format!(
        "parse_json({})",
        suggested_expression.replace("(s,", "(raw,")
    );
    assert_payload(expression(&source, &mut session).await.unwrap());
}
