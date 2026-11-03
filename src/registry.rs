use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, bail};
use futures_util::future::join_all;
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, PaginatedRequestParams, Tool},
    service::{RunningService, ServiceError},
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde::Serialize;
use serde_json::Value;

use crate::config::{Config, ServerConfig};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DISCOVERY_PAGES: usize = 100;
const MAX_DISCOVERED_TOOLS: usize = 10_000;

type Client = RunningService<RoleClient, ()>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub annotations: Option<Value>,
}

impl From<Tool> for ToolDefinition {
    fn from(tool: Tool) -> Self {
        Self {
            name: tool.name.into_owned(),
            description: tool.description.map(|text| text.into_owned()),
            input_schema: Value::Object((*tool.input_schema).clone()),
            output_schema: tool
                .output_schema
                .map(|schema| Value::Object((*schema).clone())),
            annotations: tool.annotations.map(|annotations| {
                serde_json::to_value(annotations).expect("MCP annotations are JSON serializable")
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    Ready,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServerSnapshot {
    pub status: ServerStatus,
    pub tools: BTreeMap<String, ToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct Downstream {
    client: Option<Client>,
    tools: BTreeMap<String, ToolDefinition>,
    error: Option<String>,
}

#[derive(Default)]
pub struct Registry {
    servers: BTreeMap<String, Downstream>,
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("unknown server {0:?}")]
    UnknownServer(String),
    #[error("unknown tool {tool:?} on server {server:?}")]
    UnknownTool { server: String, tool: String },
    #[error("downstream unavailable: {0}")]
    Unavailable(String),
    #[error("downstream protocol/transport error: {0}")]
    Protocol(#[from] ServiceError),
    #[error("downstream call timed out")]
    Timeout,
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("unsupported or invalid tool schema: {0}")]
    InvalidSchema(String),
    #[error("downstream returned invalid structured output: {message}")]
    InvalidResult {
        message: String,
        result: Box<CallToolResult>,
    },
    #[error("downstream tool reported an error")]
    ToolReported(Box<CallToolResult>),
}

impl Registry {
    pub async fn connect(config: Config, timeout: Duration) -> Self {
        let entries = join_all(
            config
                .mcp_servers
                .into_iter()
                .map(|(name, config)| async move {
                    let result = tokio::time::timeout(timeout, async {
                        let client = connect_server(config).await?;
                        let tools = discover(&client).await?;
                        Ok::<_, anyhow::Error>((client, tools))
                    })
                    .await;
                    let downstream = match result {
                        Ok(Ok((client, tools))) => Downstream {
                            client: Some(client),
                            tools,
                            error: None,
                        },
                        Ok(Err(error)) => failed(&name, format!("{error:#}")),
                        Err(_) => failed(&name, "connection or tool discovery timed out".into()),
                    };
                    (name, downstream)
                }),
        )
        .await;
        Self {
            servers: entries.into_iter().collect(),
        }
    }

    /// Register an already-connected client, including an in-process transport.
    pub async fn insert_client(&mut self, name: String, client: Client, timeout: Duration) {
        let downstream = match tokio::time::timeout(timeout, discover(&client)).await {
            Ok(Ok(tools)) => Downstream {
                client: Some(client),
                tools,
                error: None,
            },
            Ok(Err(error)) => failed(&name, format!("{error:#}")),
            Err(_) => failed(&name, "tool discovery timed out".into()),
        };
        self.servers.insert(name, downstream);
    }

    pub fn snapshot(&self) -> BTreeMap<String, ServerSnapshot> {
        self.servers
            .iter()
            .map(|(name, server)| {
                let ready = server
                    .client
                    .as_ref()
                    .is_some_and(|client| !client.is_closed());
                (
                    name.clone(),
                    ServerSnapshot {
                        status: if ready {
                            ServerStatus::Ready
                        } else {
                            ServerStatus::Unavailable
                        },
                        tools: server.tools.clone(),
                        error: server
                            .error
                            .clone()
                            .or_else(|| (!ready).then(|| "downstream disconnected".into())),
                    },
                )
            })
            .collect()
    }

    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<CallToolResult, CallError> {
        let downstream = self
            .servers
            .get(server)
            .ok_or_else(|| CallError::UnknownServer(server.into()))?;
        let client = downstream
            .client
            .as_ref()
            .filter(|client| !client.is_closed())
            .ok_or_else(|| {
                CallError::Unavailable(
                    downstream
                        .error
                        .clone()
                        .unwrap_or_else(|| "downstream disconnected".into()),
                )
            })?;
        let definition = downstream
            .tools
            .get(tool)
            .ok_or_else(|| CallError::UnknownTool {
                server: server.into(),
                tool: tool.into(),
            })?;
        let validator = jsonschema::validator_for(&definition.input_schema)
            .map_err(|error| CallError::InvalidSchema(error.to_string()))?;
        validator
            .validate(&arguments)
            .map_err(|error| CallError::InvalidArguments(error.to_string()))?;
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| CallError::InvalidArguments("expected an object".into()))?;
        let result = tokio::time::timeout(
            timeout,
            client.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments)),
        )
        .await
        .map_err(|_| CallError::Timeout)??;
        if result.is_error == Some(true) {
            return Err(CallError::ToolReported(Box::new(result)));
        }
        if let Some(schema) = &definition.output_schema {
            let validator = jsonschema::validator_for(schema)
                .map_err(|error| CallError::InvalidSchema(error.to_string()))?;
            let error = match &result.structured_content {
                Some(value) => validator
                    .validate(value)
                    .err()
                    .map(|error| error.to_string()),
                None => Some("outputSchema is declared but structuredContent is absent".into()),
            };
            if let Some(message) = error {
                return Err(CallError::InvalidResult {
                    message,
                    result: Box::new(result),
                });
            }
        }
        Ok(result)
    }

    pub async fn shutdown(&mut self) {
        join_all(
            self.servers
                .values_mut()
                .filter_map(|server| server.client.take())
                .map(|mut client| async move {
                    if let Err(error) = client.close_with_timeout(Duration::from_secs(5)).await {
                        tracing::warn!(%error, "downstream shutdown failed");
                    }
                }),
        )
        .await;
    }
}

fn failed(name: &str, error: String) -> Downstream {
    tracing::warn!(server = name, %error, "downstream unavailable");
    Downstream {
        client: None,
        tools: BTreeMap::new(),
        error: Some(error),
    }
}

async fn connect_server(config: ServerConfig) -> anyhow::Result<Client> {
    match config {
        ServerConfig::Stdio { command, args, env } => {
            let mut process = tokio::process::Command::new(&command);
            process.args(args).envs(env);
            let (transport, _) = TokioChildProcess::builder(process)
                .stderr(Stdio::inherit())
                .spawn()
                .with_context(|| format!("could not spawn {command:?}"))?;
            ().serve(transport).await.context("MCP handshake failed")
        }
        ServerConfig::Http { url } => {
            ().serve(StreamableHttpClientTransport::from_uri(url))
                .await
                .context("HTTP MCP handshake failed")
        }
    }
}

async fn discover(client: &Client) -> anyhow::Result<BTreeMap<String, ToolDefinition>> {
    let mut tools = BTreeMap::new();
    let mut cursor = None;
    let mut cursors = BTreeSet::new();
    for _ in 0..MAX_DISCOVERY_PAGES {
        let page = client
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .context("tools/list failed")?;
        for tool in page.tools {
            let name = tool.name.to_string();
            if tools.insert(name.clone(), tool.into()).is_some() {
                bail!("tools/list returned duplicate tool {name:?}");
            }
            if tools.len() > MAX_DISCOVERED_TOOLS {
                bail!("tools/list exceeded {MAX_DISCOVERED_TOOLS} tools");
            }
        }
        cursor = page.next_cursor;
        match &cursor {
            None => return Ok(tools),
            Some(cursor) if !cursors.insert(cursor.clone()) => {
                bail!("tools/list repeated a pagination cursor")
            }
            Some(_) => {}
        }
    }
    bail!("tools/list exceeded {MAX_DISCOVERY_PAGES} pages")
}
