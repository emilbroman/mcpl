use std::{collections::BTreeMap, sync::Arc};

pub use crate::session::SessionState;
use crate::{
    disclosure::{self, DisclosureLimits},
    frontend::{Category, Diagnostic, Span, check_expression, check_program, render_diagnostics},
    help::render_help,
    registry::Registry,
    runtime::{Code, Evaluator},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::{Value as Json, json};
use tokio::sync::Mutex;

pub const RUN_DESCRIPTION: &str = r##"Run an MCPL program. MCPL is a small statically typed scripting language for calling and combining the tools of the connected MCP servers without their results passing through you.

A program is a sequence of statements, each ending in `;`: either `name = expression;` or `expression;`. Every statement creates a binding that persists for this session; unnamed ones are called res1, res2, …. The result lists each new binding as `name: type (size) = truncated preview`. Later programs refer to bindings by name to drill in (`res1[0].title;`). A name can never be reassigned.

Values are JSON (keys may be unquoted, trailing commas allowed); raw strings `r"..."` take no escapes — use them for regexes. Call a tool as `server.tool({...args})`; tools without an output schema return `Content[]`, and `text(x)` gives their text; use `parse_json(text(x))` when that text is JSON. Fields: `x.key` or `x["key"]`; items: `x[i]`. Lambdas: `(x) => expr`. Inside parentheses or other brackets, `;` sequences expressions and `name = expr;` makes a local: `(x = f(); g(x))`. Operators `+ - * / == != < <= > >= && || !` must have spaces around them (names may contain `-`); `c ? a : b`; `+` also joins strings. Conditions must be boolean; there are no implicit conversions and no loops — iterate with built-ins like map, filter, reduce. Any error (bad index, failed tool call, exceeded limit) stops the program; earlier bindings are kept.

Use `help` to list servers, tools, signatures and built-ins, and `show` to read a value in full."##;

pub const HELP_DESCRIPTION: &str = r##"Discover MCPL servers, tools and built-ins without evaluating a program. Omit topic, or pass an empty string, for servers and built-in groups. Pass a server name for its tools, server.tool for a tool's full signature and description, or a built-in name for its signature and documentation. Output is not preview-truncated, but is capped at 64 KB. This tool creates no bindings."##;

pub const SHOW_DESCRIPTION: &str = r##"Evaluate one MCPL expression against this session and display its value without creating a binding. Use stored names and field or index access to inspect results. Optional depth, max_items, max_string and max_output control disclosure; output is capped at 64 KB. Binding lookups count as use for eviction. Expressions may call tools and have their usual effects. Functions, servers and tools display only their type."##;

pub fn run_tool() -> Tool {
    tool(
        "run",
        RUN_DESCRIPTION,
        json!({"type":"object","properties":{"program":{"type":"string"}},"required":["program"],"additionalProperties":false}),
    )
}
pub fn help_tool() -> Tool {
    tool(
        "help",
        HELP_DESCRIPTION,
        json!({"type":"object","properties":{"topic":{"type":"string"}},"additionalProperties":false}),
    )
}
pub fn show_tool() -> Tool {
    tool(
        "show",
        SHOW_DESCRIPTION,
        json!({"type":"object","properties":{
        "expression":{"type":"string"},
        "depth":{"type":"integer","minimum":0,"maximum":64},
        "max_items":{"type":"integer","minimum":0,"maximum":100000},
        "max_string":{"type":"integer","minimum":0,"maximum":16777216},
        "max_output":{"type":"integer","minimum":256,"maximum":65536}
    },"required":["expression"],"additionalProperties":false}),
    )
}
fn tool(name: &'static str, description: &'static str, schema: Json) -> Tool {
    Tool::new(
        name,
        description,
        Arc::new(schema.as_object().unwrap().clone()),
    )
}

pub struct ProxyServer {
    pub registry: Arc<Registry>,
    pub session: Mutex<SessionState>,
    validators: BTreeMap<String, jsonschema::Validator>,
}
impl ProxyServer {
    pub fn new(registry: Arc<Registry>) -> Self {
        let session = Mutex::new(SessionState::new(&registry));
        let validators = [run_tool(), help_tool(), show_tool()]
            .into_iter()
            .map(|tool| {
                (
                    tool.name.to_string(),
                    jsonschema::validator_for(&tool.schema_as_json_value()).expect("static schema"),
                )
            })
            .collect();
        Self {
            registry,
            session,
            validators,
        }
    }

    pub async fn execute(&self, name: &str, arguments: Json) -> Result<CallToolResult, ErrorData> {
        let validator = self
            .validators
            .get(name)
            .ok_or_else(|| ErrorData::invalid_params(format!("unknown tool {name:?}"), None))?;
        if let Err(error) = validator.validate(&arguments) {
            let source = arguments
                .get("program")
                .or_else(|| arguments.get("expression"))
                .and_then(Json::as_str)
                .unwrap_or("");
            let diagnostic = Diagnostic::new(
                Category::Type,
                format!("invalid {name} arguments: {error}"),
                Span::default(),
            );
            return Ok(response(diagnostic.render(source), true));
        }
        let mut session = self.session.lock().await;
        if name == "help" {
            return Ok(
                match render_help(
                    arguments.get("topic").and_then(Json::as_str),
                    &self.registry,
                    &session.environment,
                ) {
                    Ok(text) => response(text, false),
                    Err(error) => response(
                        error.render(arguments.get("topic").and_then(Json::as_str).unwrap_or("")),
                        true,
                    ),
                },
            );
        }
        let source = arguments[if name == "run" {
            "program"
        } else {
            "expression"
        }]
        .as_str()
        .unwrap();
        if !session.initialization_errors.is_empty() {
            return Ok(response(
                render_diagnostics(&session.initialization_errors, source),
                true,
            ));
        }
        if name == "show" {
            let checked = match check_expression(source, &session.environment) {
                Ok(checked) => checked,
                Err(errors) => return Ok(response(render_diagnostics(&errors, source), true)),
            };
            let code = Arc::new(
                Code::new(Arc::from(source), checked.expressions)
                    .with_definitions(session.environment.definitions.clone()),
            );
            let mut evaluator = Evaluator::new(&self.registry, &mut *session);
            return Ok(match evaluator.eval(&checked.expression, code).await {
                Ok(value) => {
                    let count = |key: &str, default| {
                        arguments
                            .get(key)
                            .and_then(Json::as_u64)
                            .map_or(default, |n| n as usize)
                    };
                    let limits = DisclosureLimits {
                        depth: count("depth", 64),
                        max_items: count("max_items", 100000),
                        max_string: count("max_string", 16777216),
                        max_output: count("max_output", 65536),
                    };
                    response(disclosure::show(&value, limits), false)
                }
                Err(error) => response(error.render(source), true),
            });
        }
        let checked = match check_program(source, &session.environment) {
            Ok(checked) => checked,
            Err(errors) => {
                return Ok(response(
                    cap_text(&render_diagnostics(&errors, source), 8192),
                    true,
                ));
            }
        };
        let code = Arc::new(
            Code::new(Arc::from(source), checked.expressions)
                .with_definitions(session.environment.definitions.clone()),
        );
        let mut evaluator = Evaluator::new(&self.registry, &mut *session);
        let mut lines = Vec::new();
        let mut completed = 0;
        let mut output_bytes = 0;
        let mut error_text = None;
        for (index, statement) in checked.statements.iter().enumerate() {
            let result = evaluator
                .eval(&statement.statement.expression, code.clone())
                .await;
            let result = result.and_then(|value| {
                evaluator.store_mut().commit(statement, value.clone())?;
                Ok(value)
            });
            match result {
                Ok(value) => {
                    completed += 1;
                    if output_bytes < 8192 {
                        let line = disclosure::binding_line(
                            &statement.name,
                            &statement.binding.display_type,
                            &value,
                        );
                        output_bytes += line.len() + 1;
                        lines.push(line);
                    }
                }
                Err(mut error) => {
                    if error.category == Category::Limit {
                        error
                            .message
                            .push_str(&format!(", statement {}", index + 1));
                    }
                    error_text = Some(error.render(source));
                    break;
                }
            }
        }
        Ok(response(
            run_output(&lines, completed, error_text.as_deref()),
            error_text.is_some(),
        ))
    }
}

fn response(text: String, error: bool) -> CallToolResult {
    let content = vec![ContentBlock::text(text)];
    if error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}
fn cap_text(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.into();
    }
    let marker = "\n… (output limit)";
    let mut end = cap.saturating_sub(marker.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{marker}", &text[..end])
}
fn run_output(lines: &[String], completed: usize, error: Option<&str>) -> String {
    let diagnostic = error.map(|text| cap_text(text, 7000));
    let available = 8192 - diagnostic.as_ref().map_or(0, |s| s.len() + 1);
    let mut out = String::new();
    let mut included = 0;
    for line in lines {
        let remaining = completed - included - 1;
        let marker = if remaining > 0 {
            format!("\n…+{remaining} bindings (output limit; use show)")
        } else {
            String::new()
        };
        if out.len() + usize::from(!out.is_empty()) + line.len() + marker.len() > available {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        included += 1;
    }
    if included < completed {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "…+{} bindings (output limit; use show)",
            completed - included
        ));
    }
    if let Some(diagnostic) = diagnostic {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&diagnostic);
    }
    out
}

impl ServerHandler for ProxyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("mcpl", env!("CARGO_PKG_VERSION")))
            .with_instructions("Use help to discover connected servers and built-ins, run to compose their tools, and show to inspect stored results.")
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![run_tool(), help_tool(), show_tool()],
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        tokio::select! {
            biased;
            _ = context.ct.cancelled() => Err(ErrorData::internal_error("request cancelled", None)),
            result = self.execute(&request.name, Json::Object(request.arguments.unwrap_or_default())) => result.map(Into::into),
        }
    }
}
