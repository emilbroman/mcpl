use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use mcpl::{
    config::Config,
    registry::{CallError, Registry, ServerStatus},
    server::ProxyServer,
};
use rmcp::{
    ErrorData, RoleClient, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    },
    service::{RequestContext, RunningService},
};
use serde_json::{Value, json};
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(10);
const SERVER: &str = "fake-server";
const TOOL: &str = "echo-value";

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("test operation timed out")
}

fn definition(name: &str) -> Tool {
    Tool::new(
        name.to_owned(),
        "Echo a value without losing MCP metadata",
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "additionalProperties": false
        })
        .as_object()
        .unwrap()
        .clone(),
    )
    .with_raw_output_schema(Arc::new(
        json!({
            "type": "object",
            "properties": {"echo": {"type": "string"}},
            "required": ["echo"],
            "additionalProperties": false
        })
        .as_object()
        .unwrap()
        .clone(),
    ))
    .with_annotations(
        ToolAnnotations::with_title("Echo value")
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

fn success() -> CallToolResult {
    let mut result = CallToolResult::structured(json!({"echo": "hello"}));
    result.content = vec![ContentBlock::text("a separate text representation")];
    result.meta = Some(
        json!({"trace": "preserve-me"})
            .as_object()
            .unwrap()
            .clone()
            .into(),
    );
    result
}

#[derive(Clone, Copy, Default)]
enum Pages {
    #[default]
    Single,
    Paginated,
    Duplicate,
    RepeatedCursor,
}

#[derive(Clone, Default)]
struct FakeServer {
    pages: Pages,
    calls: Arc<AtomicUsize>,
    started: Arc<Notify>,
    cancelled: Arc<Notify>,
}

impl ServerHandler for FakeServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.protocol_version = ProtocolVersion::V_2025_06_18;
        info
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let cursor = request.and_then(|request| request.cursor);
        let (tools, next_cursor) = match (self.pages, cursor.as_deref()) {
            (Pages::Single, None) => (vec![definition(TOOL)], None),
            (_, None) => (vec![definition(TOOL)], Some("next-page".into())),
            (Pages::Paginated, Some("next-page")) => (vec![definition("second-tool")], None),
            (Pages::Duplicate, Some("next-page")) => (vec![definition(TOOL)], None),
            (Pages::RepeatedCursor, Some("next-page")) => (vec![], Some("next-page".into())),
            _ => {
                return Err(ErrorData::invalid_params(
                    "unexpected discovery cursor",
                    None,
                ));
            }
        };
        Ok(ListToolsResult {
            tools,
            next_cursor,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.name.as_ref(), TOOL);
        let value = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("value"))
            .and_then(Value::as_str)
            .unwrap();
        let result = match value {
            "tool-error" => {
                CallToolResult::error(vec![ContentBlock::text("upstream rejected the operation")])
            }
            "invalid-output" => CallToolResult::structured(json!({"echo": 42})),
            "missing-output" => {
                CallToolResult::success(vec![ContentBlock::text("no structured output")])
            }
            "protocol-error" => {
                return Err(ErrorData::internal_error("downstream protocol error", None));
            }
            "hang" => {
                self.started.notify_one();
                context.ct.cancelled().await;
                self.cancelled.notify_one();
                return Err(ErrorData::internal_error("cancelled", None));
            }
            _ => success(),
        };
        Ok(result.into())
    }
}

async fn connect<S: ServerHandler + 'static>(
    handler: S,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, S>,
) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (server, client) =
        bounded(async { tokio::join!(handler.serve(server_io), ().serve(client_io)) }).await;
    (
        client.expect("client handshake"),
        server.expect("server handshake"),
    )
}

async fn registry(fake: FakeServer) -> (Registry, RunningService<RoleServer, FakeServer>) {
    let (client, server) = connect(fake).await;
    let mut registry = Registry::default();
    bounded(registry.insert_client(SERVER.into(), client, WAIT)).await;
    (registry, server)
}

async fn shutdown(mut registry: Registry, mut server: RunningService<RoleServer, FakeServer>) {
    bounded(registry.shutdown()).await;
    bounded(server.close_with_timeout(WAIT)).await.unwrap();
}

fn args(value: &str) -> Value {
    json!({"value": value})
}

#[tokio::test]
async fn discovers_metadata_and_exposes_only_run_with_a_structured_listing() {
    let config = Config::parse(
        r#"{"mcpServers":{"broken-server":{"command":"/mcpl-test-missing-executable"}}}"#,
    )
    .unwrap();
    let mut registry = bounded(Registry::connect(config, WAIT)).await;
    let (downstream_client, downstream) = connect(FakeServer::default()).await;
    bounded(registry.insert_client(SERVER.into(), downstream_client, WAIT)).await;
    let snapshot = registry.snapshot();
    assert_eq!(snapshot["broken-server"].status, ServerStatus::Unavailable);
    assert!(
        snapshot["broken-server"]
            .error
            .as_deref()
            .unwrap()
            .contains("could not spawn")
    );
    let server = &snapshot[SERVER];
    assert_eq!(server.status, ServerStatus::Ready);
    assert!(server.error.is_none());
    let tool = &server.tools[TOOL];
    let expected = definition(TOOL);
    assert_eq!(tool.name, TOOL);
    assert_eq!(tool.description.as_deref(), expected.description.as_deref());
    assert_eq!(
        tool.input_schema,
        Value::Object((*expected.input_schema).clone())
    );
    assert_eq!(
        tool.output_schema,
        expected.output_schema.map(|s| Value::Object((*s).clone()))
    );
    assert_eq!(
        tool.annotations,
        expected
            .annotations
            .map(|a| serde_json::to_value(a).unwrap())
    );

    let shared = Arc::new(registry);
    let (mut client, mut proxy) = connect(ProxyServer::new(shared.clone())).await;
    let tools = bounded(client.list_tools(None)).await.unwrap();
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "run");
    let result = bounded(
        client.call_tool(
            CallToolRequestParams::new("run").with_arguments(
                json!({"program": "this is deliberately not executed"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ),
    )
    .await
    .unwrap();
    let listing = result.structured_content.expect("structured listing");
    assert_eq!(listing["stub"], true);
    let servers = listing["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["name"], "broken-server");
    assert_eq!(servers[0]["status"], "unavailable");
    assert_eq!(servers[0]["tools"], json!([]));
    assert!(
        servers[0]["error"]
            .as_str()
            .unwrap()
            .contains("could not spawn")
    );
    assert_eq!(
        servers[1],
        json!({
            "name": SERVER, "status": "ready", "tools": [TOOL], "error": null
        })
    );
    assert_ne!(result.is_error, Some(true));

    bounded(client.close_with_timeout(WAIT)).await.unwrap();
    bounded(proxy.close_with_timeout(WAIT)).await.unwrap();
    drop(proxy);
    drop(client);
    let registry =
        Arc::try_unwrap(shared).unwrap_or_else(|_| panic!("proxy retained registry after close"));
    shutdown(registry, downstream).await;
}

#[tokio::test]
async fn run_validates_program_and_rejects_unknown_tools() {
    let (mut client, mut proxy) = connect(ProxyServer::new(Arc::new(Registry::default()))).await;
    for arguments in [
        json!({}),
        json!({"program": 12}),
        json!({"program": null}),
        json!({"program": "ok", "unexpected": true}),
    ] {
        let result = bounded(
            client.call_tool(
                CallToolRequestParams::new("run")
                    .with_arguments(arguments.as_object().unwrap().clone()),
            ),
        )
        .await;
        assert!(
            result.is_err(),
            "accepted invalid run arguments: {arguments}"
        );
    }
    assert!(
        bounded(client.call_tool(CallToolRequestParams::new(TOOL)))
            .await
            .is_err()
    );
    let result = bounded(
        client.call_tool(
            CallToolRequestParams::new("run")
                .with_arguments(json!({"program": ""}).as_object().unwrap().clone()),
        ),
    )
    .await
    .unwrap();
    assert_eq!(result.structured_content.unwrap()["servers"], json!([]));
    bounded(client.close_with_timeout(WAIT)).await.unwrap();
    bounded(proxy.close_with_timeout(WAIT)).await.unwrap();
}

#[tokio::test]
async fn dispatch_preserves_full_results_and_distinguishes_failures() {
    let fake = FakeServer::default();
    let calls = fake.calls.clone();
    let (registry, downstream) = registry(fake).await;
    let result = bounded(registry.call_tool(SERVER, TOOL, args("hello"), WAIT))
        .await
        .unwrap();
    let expected = success();
    assert_eq!(result.content, expected.content);
    assert_eq!(result.structured_content, expected.structured_content);
    assert_eq!(result.meta, expected.meta);
    assert_eq!(result.is_error, expected.is_error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    assert!(
        matches!(bounded(registry.call_tool("missing-server", TOOL, args("hello"), WAIT)).await,
        Err(CallError::UnknownServer(name)) if name == "missing-server")
    );
    assert!(
        matches!(bounded(registry.call_tool(SERVER, "missing-tool", args("hello"), WAIT)).await,
        Err(CallError::UnknownTool { server, tool }) if server == SERVER && tool == "missing-tool")
    );
    for arguments in [
        json!({}),
        json!({"value": 4}),
        json!([]),
        json!(null),
        json!({"value": "ok", "extra": true}),
    ] {
        assert!(matches!(
            bounded(registry.call_tool(SERVER, TOOL, arguments, WAIT)).await,
            Err(CallError::InvalidArguments(_))
        ));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "invalid calls reached the downstream"
    );

    match bounded(registry.call_tool(SERVER, TOOL, args("tool-error"), WAIT)).await {
        Err(CallError::ToolReported(result)) => {
            assert_eq!(result.is_error, Some(true));
            assert_eq!(
                result.content,
                vec![ContentBlock::text("upstream rejected the operation")]
            );
        }
        other => panic!("expected tool-level error, got {other:?}"),
    }
    for value in ["invalid-output", "missing-output"] {
        match bounded(registry.call_tool(SERVER, TOOL, args(value), WAIT)).await {
            Err(CallError::InvalidResult { result, message }) => {
                assert!(!message.is_empty());
                if value == "invalid-output" {
                    assert_eq!(result.structured_content, Some(json!({"echo": 42})));
                } else {
                    assert_eq!(
                        result.content,
                        vec![ContentBlock::text("no structured output")]
                    );
                }
            }
            other => panic!("expected invalid output for {value}, got {other:?}"),
        }
    }
    assert!(matches!(
        bounded(registry.call_tool(SERVER, TOOL, args("protocol-error"), WAIT)).await,
        Err(CallError::Protocol(_))
    ));
    assert!(
        bounded(registry.call_tool(SERVER, TOOL, args("hello"), WAIT))
            .await
            .is_ok()
    );
    shutdown(registry, downstream).await;
}

#[tokio::test]
async fn timeout_and_abandoned_calls_cancel_the_downstream_request() {
    let fake = FakeServer::default();
    let started = fake.started.clone();
    let cancelled = fake.cancelled.clone();
    let (registry, downstream) = registry(fake).await;
    let error =
        bounded(registry.call_tool(SERVER, TOOL, args("hang"), Duration::from_millis(200))).await;
    assert!(matches!(error, Err(CallError::Timeout)));
    bounded(started.notified()).await;
    bounded(cancelled.notified()).await;

    {
        let mut call = Box::pin(registry.call_tool(SERVER, TOOL, args("hang"), WAIT));
        bounded(async {
            tokio::select! {
                result = &mut call => panic!("hanging call completed: {result:?}"),
                _ = started.notified() => {}
            }
        })
        .await;
        drop(call);
    }
    bounded(cancelled.notified()).await;
    assert!(
        bounded(registry.call_tool(SERVER, TOOL, args("hello"), WAIT))
            .await
            .is_ok()
    );
    shutdown(registry, downstream).await;
}

#[tokio::test]
async fn discovery_follows_pagination_without_renaming_tools() {
    let (registry, downstream) = registry(FakeServer {
        pages: Pages::Paginated,
        ..Default::default()
    })
    .await;
    let snapshot = registry.snapshot();
    assert_eq!(snapshot[SERVER].status, ServerStatus::Ready);
    assert_eq!(
        snapshot[SERVER]
            .tools
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![TOOL, "second-tool"]
    );
    shutdown(registry, downstream).await;
}

#[tokio::test]
async fn invalid_discovery_is_unavailable_not_a_partial_tool_catalogue() {
    for (pages, message) in [
        (Pages::Duplicate, "duplicate tool"),
        (Pages::RepeatedCursor, "repeated a pagination cursor"),
    ] {
        let (registry, downstream) = registry(FakeServer {
            pages,
            ..Default::default()
        })
        .await;
        let snapshot = registry.snapshot();
        let server = &snapshot[SERVER];
        assert_eq!(server.status, ServerStatus::Unavailable);
        assert!(server.tools.is_empty());
        assert!(server.error.as_deref().unwrap().contains(message));
        assert!(matches!(
            bounded(registry.call_tool(SERVER, TOOL, args("hello"), WAIT)).await,
            Err(CallError::Unavailable(_))
        ));
        shutdown(registry, downstream).await;
    }
}
