use std::sync::Arc;

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::registry::Registry;

pub const RUN_DESCRIPTION: &str = "Run an MCPL program. Language execution is not implemented yet: this stub ignores the program and lists discovered downstream servers and tool names, including connection errors.";

pub fn run_tool() -> Tool {
    Tool::new(
        "run",
        RUN_DESCRIPTION,
        Arc::new(
            json!({
                "type": "object",
                "properties": { "program": { "type": "string" } },
                "required": ["program"],
                "additionalProperties": false
            })
            .as_object()
            .expect("static object schema")
            .clone(),
        ),
    )
}

// The interpreter will add its handle store here. Construct a new ProxyServer per connection.
#[derive(Default)]
pub struct SessionState {}

pub struct ProxyServer {
    pub registry: Arc<Registry>,
    pub session: Mutex<SessionState>,
    run_validator: jsonschema::Validator,
}

impl ProxyServer {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self {
            registry,
            session: Mutex::new(SessionState::default()),
            run_validator: jsonschema::validator_for(&run_tool().schema_as_json_value())
                .expect("static run schema is valid"),
        }
    }
}

impl ServerHandler for ProxyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("mcpl", env!("CARGO_PKG_VERSION")))
            .with_instructions("MCPL proxy core. Call run with a program string to inspect downstream tool availability. The program is not executed yet.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![run_tool()],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != "run" {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {:?}", request.name),
                None,
            ));
        }
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        self.run_validator.validate(&arguments).map_err(|error| {
            ErrorData::invalid_params(format!("invalid run arguments: {error}"), None)
        })?;
        let servers: Vec<_> = self
            .registry
            .snapshot()
            .into_iter()
            .map(|(name, server)| {
                json!({ "name": name, "status": server.status,
                "tools": server.tools.into_keys().collect::<Vec<_>>(), "error": server.error })
            })
            .collect();
        Ok(CallToolResult::structured(json!({
            "stub": true,
            "message": "Program not executed; MCPL language support is not implemented yet.",
            "servers": servers
        }))
        .into())
    }
}
