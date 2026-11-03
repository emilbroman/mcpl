use std::sync::{Arc, Mutex};

use mcpl::{
    registry::Registry,
    server::{ProxyServer, SessionState},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Fake {
    current: Arc<Mutex<String>>,
    calls: Arc<Mutex<Vec<String>>>,
}
fn tool(
    name: &'static str,
    description: &'static str,
    input: Value,
    output: Option<Value>,
) -> Tool {
    let tool = Tool::new(name, description, input.as_object().unwrap().clone());
    if let Some(output) = output {
        tool.with_raw_output_schema(Arc::new(output.as_object().unwrap().clone()))
    } else {
        tool
    }
}
impl ServerHandler for Fake {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let empty = json!({"type":"object","properties":{},"additionalProperties":false});
        Ok(ListToolsResult {
            tools: vec![
                tool(
                    "browser_navigate",
                    "Navigate to a URL.",
                    json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}),
                    None,
                ),
                tool(
                    "browser_snapshot",
                    "Capture a snapshot.",
                    empty.clone(),
                    None,
                ),
            ],
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.lock().unwrap().push(request.name.to_string());
        let result = if request.name == "browser_navigate" {
            let args = request.arguments.unwrap();
            let url = args["url"].as_str().unwrap();
            if url == "bad" {
                CallToolResult::error(vec![ContentBlock::text("navigation failed")])
            } else {
                *self.current.lock().unwrap() = url.to_owned();
                CallToolResult::success(vec![])
            }
        } else {
            let current = self.current.lock().unwrap();
            let text = if current.as_str() == "large" {
                "x".repeat(16 * 1024 * 1024 + 1)
            } else if current.as_str() == "https://e.test" {
                "Page title: Index\n[A](https://e.test/a)\n[B](https://e.test/b)\n[C](https://e.test/c)\n[D](https://e.test/d)\n[E](https://e.test/e)\n[F](https://e.test/f)".to_owned()
            } else {
                format!(
                    "Page title: {}",
                    current.chars().last().unwrap_or('a').to_uppercase()
                )
            };
            CallToolResult::success(vec![ContentBlock::text(text)])
        };
        Ok(result.into())
    }
}
async fn fixture() -> (
    ProxyServer,
    Fake,
    rmcp::service::RunningService<RoleServer, Fake>,
) {
    let fake = Fake::default();
    let (a, b) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(a);
    let transport = rmcp::transport::async_rw::AsyncRwTransport::new_client(read, write)
        .with_max_line_length(32 * 1024 * 1024);
    let (client, server) = tokio::join!(().serve(transport), fake.clone().serve(b));
    let mut registry = Registry::default();
    registry
        .insert_client("playwright".into(), client.unwrap())
        .await;
    (ProxyServer::new(Arc::new(registry)), fake, server.unwrap())
}
async fn invoke(server: &ProxyServer, name: &str, args: Value) -> (bool, String) {
    let result = server.execute(name, args).await.unwrap();
    (
        result.is_error == Some(true),
        result
            .content
            .iter()
            .filter_map(|b| b.as_text())
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
async fn run(server: &ProxyServer, source: &str) -> String {
    let (error, text) = invoke(server, "run", json!({"program":source})).await;
    assert!(!error, "{text}");
    text
}
fn blocks(section: &str, language: &str) -> Vec<String> {
    section
        .split(&format!("```{language}\n"))
        .skip(1)
        .map(|s| s.split("\n```").next().unwrap().to_owned())
        .collect()
}
#[tokio::test]
async fn spec_playwright_examples_are_verbatim_and_calls_sequential() {
    let (server, fake, mut downstream) = fixture().await;
    let spec = include_str!("../docs/language.md");
    let a = spec
        .split("### A. First-contact discovery")
        .nth(1)
        .unwrap()
        .split("### B.")
        .next()
        .unwrap();
    assert_eq!(
        run(&server, &blocks(a, "mcpl")[0]).await,
        blocks(a, "text")[1]
    );
    assert!(fake.calls.lock().unwrap().is_empty());
    let (_, help) = invoke(
        &server,
        "help",
        json!({"topic":"playwright.browser_navigate"}),
    )
    .await;
    assert_eq!(
        help,
        "playwright.browser_navigate: (args: {url: string}) => Content[]\nNavigate to a URL."
    );
    let server = ProxyServer::new(server.registry.clone());
    let b = spec
        .split("### B. Playwright aggregation")
        .nth(1)
        .unwrap()
        .split("### C.")
        .next()
        .unwrap();
    let programs = blocks(b, "mcpl");
    let texts = blocks(b, "text");
    assert_eq!(run(&server, &programs[0]).await, texts[1]);
    assert_eq!(run(&server, &programs[1]).await, texts[2]);
    assert_eq!(run(&server, &programs[3]).await, texts[3]);
    let (error, text) = invoke(
        &server,
        "show",
        json!({"expression":"res5[1]","depth":3,"max_items":20,"max_string":500,"max_output":4096}),
    )
    .await;
    assert!(!error);
    assert_eq!(text, texts[4]);
    assert_eq!(run(&server, "1;").await, "res7: number (1 value) = 1");
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["browser_navigate", "browser_snapshot"].repeat(6)
    );
    downstream.close().await.unwrap();
}
#[tokio::test]
async fn partial_commit_preflight_and_session_isolation() {
    let (server, fake, mut downstream) = fixture().await;
    let (error, text) = invoke(
        &server,
        "run",
        json!({"program":"kept = 7; playwright.browser_navigate({url: \"bad\"}); never = 8;"}),
    )
    .await;
    assert!(error);
    assert!(text.starts_with("kept: number (1 value) = 7\nruntime error:"));
    assert!(text.contains("navigation failed"));
    assert_eq!(
        run(&server, "kept + 1;").await,
        "res1: number (1 value) = 8"
    );
    let count = fake.calls.lock().unwrap().len();
    let (error, text) = invoke(
        &server,
        "run",
        json!({"program":"playwright.browser_snapshot({}); kept = 9;"}),
    )
    .await;
    assert!(error);
    assert!(text.contains("already defined"));
    assert_eq!(fake.calls.lock().unwrap().len(), count);
    assert!(invoke(&server, "run", json!({"program":"never;"})).await.0);
    let other = ProxyServer::new(server.registry.clone());
    assert!(invoke(&other, "run", json!({"program":"kept;"})).await.0);
    assert_eq!(run(&other, "4;").await, "res1: number (1 value) = 4");
    downstream.close().await.unwrap();
}
#[tokio::test]
async fn handles_closures_shadowing_and_show_without_binding() {
    let server = ProxyServer::new(Arc::new(Registry::default()));
    assert_eq!(
        run(&server, "{x: 123};").await,
        "res1: {x: number} (1 key) = {x: 123}"
    );
    assert_eq!(
        run(&server, "{y: res1};").await,
        "res2: {y: {x: number}} (1 key) = {y: {x: 123}}"
    );
    run(
        &server,
        "a.b = {c: 9}; f = (x) => a.b.c + x; g = (text) => upper(text);",
    )
    .await;
    let (_, text) = invoke(&server, "show", json!({"expression":"f(3)"})).await;
    assert_eq!(text, "12");
    assert_eq!(
        run(&server, "g(\"yes\");").await,
        "res3: string (3 chars) = \"YES\""
    );
    assert!(invoke(&server, "show", json!({"expression":"1;"})).await.0);
}
#[tokio::test]
async fn source_collection_expression_value_call_and_fuel_caps() {
    let (server, fake, mut downstream) = fixture().await;
    for (program, needle) in [
        (" ".repeat(65537), "source"),
        ("range(100001);".into(), "collection"),
        (
            format!("{}1{};", "(".repeat(257), ")".repeat(257)),
            "nesting",
        ),
        (
            format!(
                "parse_json({});",
                serde_json::to_string(&format!("{}0{}", "[".repeat(65), "]".repeat(65))).unwrap()
            ),
            "nesting",
        ),
        (
            "map(range(51), (x) => playwright.browser_snapshot({}));".into(),
            "downstream",
        ),
        (
            "xs = range(100000); map(xs, (x) => sum(xs));".into(),
            "evaluation budget",
        ),
    ] {
        let (error, text) = invoke(&server, "run", json!({"program":program})).await;
        assert!(error, "{text}");
        assert!(text.contains("limit exceeded"), "{text}");
        assert!(text.contains(needle), "expected {needle}: {text}");
    }
    assert_eq!(fake.calls.lock().unwrap().len(), 50);
    assert!(run(&server, "len(xs);").await.contains("100000"));
    downstream.close().await.unwrap();
}
#[tokio::test]
async fn output_caps_keep_all_commits_and_errors() {
    let server = ProxyServer::new(Arc::new(Registry::default()));
    let source = (0..600).map(|i| format!("v{i} = {i};")).collect::<String>();
    let text = run(&server, &source).await;
    assert!(text.len() <= 8192);
    assert!(text.contains("bindings (output limit; use show)"));
    assert!(run(&server, "v599;").await.ends_with("599"));
    let (_, text) = invoke(
        &server,
        "show",
        json!({"expression":"range(1000)","max_output":256}),
    )
    .await;
    assert!(text.len() <= 256);
    assert!(text.ends_with(']'));
    let (error, text) = invoke(
        &server,
        "show",
        json!({"expression":"range(100001)","max_output":256}),
    )
    .await;
    assert!(error);
    assert!(text.starts_with("limit exceeded:"));
}
#[tokio::test]
async fn lru_eviction_reserves_names_and_show_refreshes_use() {
    let server = ProxyServer::new(Arc::new(Registry::default()));
    let mut state = server.session.lock().await;
    *state = SessionState::new(&server.registry).with_memory_limit(32000);
    drop(state);
    run(&server, "a = range(100); b = range(100);").await;
    assert!(
        !invoke(&server, "show", json!({"expression":"len(a)"}))
            .await
            .0
    );
    run(&server, "c = range(100);").await;
    // Tighten pressure without hard-coding internal allocation representation.
    for i in 0..8 {
        let _ = invoke(
            &server,
            "run",
            json!({"program":format!("more{i} = range(100);")}),
        )
        .await;
    }
    let (error, text) = invoke(&server, "show", json!({"expression":"b"})).await;
    assert!(error);
    assert!(text.contains("was evicted"), "{text}");
    let (error, text) = invoke(&server, "run", json!({"program":"b = 1;"})).await;
    assert!(error);
    assert!(text.contains("already defined"));
}

#[derive(Clone)]
struct Catalog {
    absent: bool,
}
impl ServerHandler for Catalog {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let empty = json!({"type":"object","properties":{},"additionalProperties":false});
        Ok(ListToolsResult {
            tools: vec![
                tool(
                    "profile",
                    "Profile",
                    empty.clone(),
                    Some(
                        json!({"type":"object","properties":{"name":{"type":"string"}},"additionalProperties":false}),
                    ),
                ),
                tool(
                    "lookup",
                    "Lookup",
                    empty,
                    Some(json!({"oneOf":[
                        {"type":"object","properties":{"kind":{"const":"ok"},"value":{"type":"number"}},"required":["kind","value"],"additionalProperties":false},
                        {"type":"object","properties":{"kind":{"const":"error"},"message":{"type":"string"}},"required":["kind","message"],"additionalProperties":false}
                    ]})),
                ),
            ],
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        r: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResult::structured(if r.name == "profile" {
            if self.absent {
                json!({})
            } else {
                json!({"name":"Ada"})
            }
        } else {
            json!({"kind":"ok","value":42})
        })
        .into())
    }
}
#[tokio::test]
async fn remaining_spec_examples_and_absent_optional_indexes() {
    let spec = include_str!("../docs/language.md");
    for absent in [false, true] {
        let (a, b) = tokio::io::duplex(65536);
        let (client, downstream) = tokio::join!(().serve(a), Catalog { absent }.serve(b));
        let mut downstream = downstream.unwrap();
        let mut registry = Registry::default();
        registry
            .insert_client("catalog".into(), client.unwrap())
            .await;
        let server = ProxyServer::new(Arc::new(registry));
        let section = spec
            .split("### C. Optional-property")
            .nth(1)
            .unwrap()
            .split("### D.")
            .next()
            .unwrap();
        let result = run(&server, &blocks(section, "mcpl")[0]).await;
        if absent {
            assert!(result.contains("label: string (9 chars) = \"anonymous\""));
            assert!(result.contains("starts_a: boolean (1 value) = false"));
        } else {
            assert_eq!(result, blocks(section, "text")[1]);
        }
        let (_, text) = invoke(&server, "show", json!({"expression":"profile[\"name\"]"})).await;
        assert_eq!(text, if absent { "null" } else { "\"Ada\"" });
        let (error, grouped) =
            invoke(&server, "show", json!({"expression":"profile[(\"name\")]"})).await;
        assert!(!error, "{grouped}");
        assert_eq!(grouped, text);
        downstream.close().await.unwrap();
    }
    for title in ["### D. Runtime failure", "### E. Locals and raw"] {
        let section = spec
            .split(title)
            .nth(1)
            .unwrap()
            .split("\n##")
            .next()
            .unwrap();
        let server = ProxyServer::new(Arc::new(Registry::default()));
        let (error, text) =
            invoke(&server, "run", json!({"program":blocks(section,"mcpl")[0]})).await;
        assert_eq!(error, title.contains("Runtime"));
        assert_eq!(text, blocks(section, "text")[0]);
    }
}

#[tokio::test]
async fn self_application_is_fuel_bounded_and_previous_binding_survives() {
    let server = ProxyServer::new(Arc::new(Registry::default()));
    let (error, text) = invoke(&server, "run", json!({"program":"f = (x) => x(x); f(f);"})).await;
    assert!(error);
    assert!(text.contains("evaluation budget exhausted"), "{text}");
    assert!(text.starts_with("f: (x: any) => any (function)"));
    let (error, _) = invoke(&server, "show", json!({"expression":"f"})).await;
    assert!(!error);
}

#[derive(Clone, Default)]
struct Slow {
    started: Arc<tokio::sync::Notify>,
    cancelled: Arc<tokio::sync::Notify>,
}
impl ServerHandler for Slow {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![tool(
                "wait",
                "Wait for cancellation",
                json!({"type":"object","additionalProperties":false}),
                None,
            )],
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.started.notify_one();
        context.ct.cancelled().await;
        self.cancelled.notify_one();
        Err(ErrorData::internal_error("cancelled", None))
    }
}
#[tokio::test]
async fn upstream_cancellation_keeps_completed_statements_and_unlocks_session() {
    let slow = Slow::default();
    let (a, b) = tokio::io::duplex(65536);
    let (client, downstream) = tokio::join!(().serve(a), slow.clone().serve(b));
    let mut downstream = downstream.unwrap();
    let mut registry = Registry::default();
    registry.insert_client("slow".into(), client.unwrap()).await;
    let registry = Arc::new(registry);
    let (a, b) = tokio::io::duplex(65536);
    let (client, proxy) = tokio::join!(().serve(a), ProxyServer::new(registry.clone()).serve(b));
    let mut client = client.unwrap();
    let mut proxy = proxy.unwrap();
    let args = json!({"program":"saved = 17; slow.wait({});"})
        .as_object()
        .unwrap()
        .clone();
    let mut request =
        Box::pin(client.call_tool(CallToolRequestParams::new("run").with_arguments(args)));
    tokio::select! { result=&mut request=>panic!("wait returned {result:?}"),_=slow.started.notified()=>{} }
    drop(request);
    slow.cancelled.notified().await;
    let result = client
        .call_tool(
            CallToolRequestParams::new("run")
                .with_arguments(json!({"program":"saved + 1;"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        "res1: number (1 value) = 18"
    );
    client.close().await.unwrap();
    proxy.close().await.unwrap();
    downstream.close().await.unwrap();
}

#[test]
fn public_descriptions_and_schemas_match_normative_spec() {
    use mcpl::server::{help_tool, run_tool, show_tool};
    let spec = include_str!("../docs/language.md");
    let run = spec
        .split("### Normative `RUN_DESCRIPTION`")
        .nth(1)
        .unwrap();
    assert_eq!(
        run_tool().description.as_deref(),
        Some(blocks(run, "text")[0].as_str())
    );
    let section = spec
        .split("## 12. The three MCP tools")
        .nth(1)
        .unwrap()
        .split("## 13.")
        .next()
        .unwrap();
    for (name, tool) in [
        ("run", run_tool()),
        ("help", help_tool()),
        ("show", show_tool()),
    ] {
        let part = section.split(&format!("### `{name}`")).nth(1).unwrap();
        let schema: Value = serde_json::from_str(&blocks(part, "json")[0]).unwrap();
        assert_eq!(tool.schema_as_json_value(), schema);
        if name != "run" {
            assert_eq!(
                tool.description.as_deref(),
                Some(blocks(part, "text")[0].as_str())
            );
        }
    }
}

#[tokio::test]
async fn oversized_downstream_string_and_session_value_fail_with_limits() {
    let (server, _, mut downstream) = fixture().await;
    let (error, text) = invoke(&server,"run",json!({"program":"playwright.browser_navigate({url: \"large\"}); playwright.browser_snapshot({});"})).await;
    assert!(error);
    assert!(text.contains("string length limit"), "{text}");
    assert!(text.starts_with("res1: Content[] (0 items) = []\nlimit exceeded:"));
    downstream.close().await.unwrap();
    let server = ProxyServer::new(Arc::new(Registry::default()));
    *server.session.lock().await = SessionState::default().with_memory_limit(32000);
    let (error, text) = invoke(&server, "run", json!({"program":"range(1000);"})).await;
    assert!(error);
    assert!(text.contains("session memory limit"), "{text}");
    assert_eq!(run(&server, "1;").await, "res1: number (1 value) = 1");
}
