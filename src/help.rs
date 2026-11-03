use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write},
    sync::LazyLock,
};

use crate::{
    frontend::{Category, Diagnostic, Environment, Span},
    registry::{Registry, ServerSnapshot, ServerStatus},
    types::Type,
};

const OUTPUT_LIMIT: usize = 65_536;
const OUTPUT_MARKER: &str = "… (output limit)";
const LANGUAGE: &str = include_str!("../docs/language.md");
static CATALOG: LazyLock<Catalog> = LazyLock::new(Catalog::from_spec);

/// Discover documentation without evaluating expressions or touching session bindings.
pub fn render_help(
    topic: Option<&str>,
    registry: &Registry,
    environment: &Environment,
) -> Result<String, Diagnostic> {
    render(topic.unwrap_or_default(), &registry.snapshot(), environment)
}

fn render(
    topic: &str,
    servers: &BTreeMap<String, ServerSnapshot>,
    environment: &Environment,
) -> Result<String, Diagnostic> {
    let mut output = Output::default();
    if topic.is_empty() {
        writeln!(output, "Servers").unwrap();
        if servers.is_empty() {
            writeln!(output, "  (none configured)").unwrap();
        }
        for (name, server) in servers {
            write!(output, "  {name}: ").unwrap();
            status(&mut output, server);
            writeln!(output).unwrap();
        }
        writeln!(output, "\nBuilt-ins").unwrap();
        for group in &CATALOG.groups {
            let names: Vec<_> = group
                .names
                .iter()
                .filter(|name| environment.builtins.contains_key(*name))
                .map(String::as_str)
                .collect();
            writeln!(output, "  {}: {}", group.name, names.join(", ")).unwrap();
        }
    } else if let Some(builtin) = environment.builtins.get(topic) {
        if let Some(documentation) = CATALOG.builtins.get(topic) {
            writeln!(
                output,
                "{}\n\n{}",
                documentation.signatures, documentation.description
            )
            .unwrap();
            writeln!(output, "\n{}", CATALOG.common).unwrap();
            writeln!(output, "\n{}", CATALOG.groups[documentation.group].notes).unwrap();
            if matches!(topic, "str" | "to_json" | "contains" | "unique") {
                writeln!(output, "\nFunctions, servers, and tools are not JSON. Containers holding them are also not JSON. to_json renders the whole value's type for such values; equality on them fails, including at runtime through any.").unwrap();
            }
        } else {
            writeln!(output, "{topic}: {}", builtin.signature).unwrap();
        }
        let ty = Type::Function(builtin.signature.clone());
        definitions(&mut output, [&ty], &environment.definitions);
    } else if let Some(server) = servers.get(topic) {
        write!(output, "{topic}: ").unwrap();
        status(&mut output, server);
        writeln!(output).unwrap();
        let mut types = Vec::new();
        for (name, tool) in &server.tools {
            if let Some(ty) = tool_type(environment, topic, name) {
                writeln!(output, "{topic}.{name}: {ty}").unwrap();
                types.push(ty);
            } else {
                writeln!(output, "{topic}.{name}: (signature unavailable)").unwrap();
            }
            if let Some(line) = tool.description.as_deref().and_then(first_line) {
                writeln!(output, "  {line}").unwrap();
            }
        }
        if server.tools.is_empty() {
            writeln!(output, "  (no tools)").unwrap();
        }
        definitions(&mut output, types, &environment.definitions);
    } else if let Some((server_name, server, name)) = resolve_tool(topic, servers)
        && let Some(tool) = server.tools.get(name)
    {
        if let Some(ty) = tool_type(environment, server_name, name) {
            writeln!(output, "{topic}: {ty}").unwrap();
            if let Some(description) = &tool.description {
                write!(output, "{description}").unwrap();
            }
            definitions(&mut output, [ty], &environment.definitions);
        } else {
            writeln!(output, "{topic}: (signature unavailable)").unwrap();
            if let Some(description) = &tool.description {
                write!(output, "{description}").unwrap();
            }
        }
    } else {
        return Err(unknown_topic(topic, servers, environment));
    }
    Ok(output.finish())
}

fn status(output: &mut Output, server: &ServerSnapshot) {
    match server.status {
        ServerStatus::Ready => write!(output, "available").unwrap(),
        ServerStatus::Unavailable => {
            write!(output, "unavailable").unwrap();
            if let Some(error) = &server.error {
                write!(output, " — {error}").unwrap();
            }
        }
    }
}

fn first_line(description: &str) -> Option<&str> {
    description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn tool_type<'a>(environment: &'a Environment, server: &str, name: &str) -> Option<&'a Type> {
    let Type::Server(tools) = environment.servers.get(server)? else {
        return None;
    };
    tools
        .iter()
        .find(|(tool, _)| tool == name)
        .map(|(_, ty)| ty)
}

fn resolve_tool<'a>(
    topic: &'a str,
    servers: &'a BTreeMap<String, ServerSnapshot>,
) -> Option<(&'a str, &'a ServerSnapshot, &'a str)> {
    servers
        .iter()
        .filter_map(|(name, server)| {
            topic
                .strip_prefix(name.as_str())?
                .strip_prefix('.')
                .map(|tool| (name.as_str(), server, tool))
        })
        .max_by_key(|(name, _, _)| name.len())
}

fn definitions<'a>(
    output: &mut Output,
    roots: impl IntoIterator<Item = &'a Type>,
    definitions: &BTreeMap<String, Type>,
) {
    let mut names = BTreeSet::new();
    let mut pending = BTreeSet::new();
    for root in roots {
        references(root, &mut pending);
    }
    while let Some(name) = pending.pop_first() {
        if names.insert(name.clone())
            && let Some(ty) = definitions.get(&name)
        {
            references(ty, &mut pending);
        }
    }
    // Content is the fixed MCP envelope, not a recursive schema definition.
    names.remove("Content");
    let mut heading = false;
    for name in names {
        if let Some(ty) = definitions.get(&name) {
            if !heading {
                writeln!(output, "\n\nDefinitions").unwrap();
                heading = true;
            }
            writeln!(output, "{name} = {ty}").unwrap();
        }
    }
}

fn references(ty: &Type, names: &mut BTreeSet<String>) {
    match ty {
        Type::Named(name) => {
            names.insert(name.clone());
        }
        Type::Array(item) => references(item, names),
        Type::Tuple(tuple) => {
            for item in &tuple.items {
                references(&item.ty, names);
            }
            if let Some(rest) = &tuple.rest {
                references(rest, names);
            }
        }
        Type::Record(record) => {
            for field in &record.fields {
                references(&field.ty, names);
            }
            if let Some(additional) = &record.additional {
                references(additional, names);
            }
        }
        Type::Union(items) => {
            for item in items {
                references(item, names);
            }
        }
        Type::Server(tools) => {
            for (_, tool) in tools {
                references(tool, names);
            }
        }
        Type::Function(function) => function_references(function, names),
        Type::Tool(tool) => function_references(&tool.signature, names),
        Type::Builtin(builtin) => function_references(&builtin.signature, names),
        _ => {}
    }
}

fn function_references(function: &crate::types::FunctionType, names: &mut BTreeSet<String>) {
    for parameter in &function.parameters {
        references(&parameter.ty, names);
    }
    references(&function.result, names);
}

fn unknown_topic(
    topic: &str,
    servers: &BTreeMap<String, ServerSnapshot>,
    environment: &Environment,
) -> Diagnostic {
    let abbreviated: String = topic.chars().take(256).collect();
    let mut candidates: Vec<_> = environment.builtins.keys().cloned().collect();
    for (name, server) in servers {
        candidates.push(name.clone());
        candidates.extend(server.tools.keys().map(|tool| format!("{name}.{tool}")));
    }
    let suggestion = candidates
        .iter()
        .min_by_key(|candidate| edit_distance(&abbreviated, candidate));
    let diagnostic = Diagnostic::new(
        Category::Type,
        format!("unknown help topic '{abbreviated}'"),
        Span::new(0, topic.len()),
    );
    match suggestion {
        Some(topic) => diagnostic.with_hint(format!("did you mean '{topic}'?")),
        None => diagnostic.with_hint("omit topic to list servers and built-ins"),
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut row: Vec<_> = (0..=left.chars().count()).collect();
    for (index, right) in right.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = index + 1;
        for (column, left) in left.chars().enumerate() {
            let old = row[column + 1];
            row[column + 1] = (diagonal + usize::from(left != right))
                .min(row[column] + 1)
                .min(old + 1);
            diagonal = old;
        }
    }
    *row.last().unwrap()
}

struct BuiltinDoc {
    signatures: String,
    description: String,
    group: usize,
}

struct Group {
    name: &'static str,
    names: Vec<String>,
    notes: &'static str,
}

struct Catalog {
    common: &'static str,
    groups: Vec<Group>,
    builtins: BTreeMap<String, BuiltinDoc>,
}

impl Catalog {
    fn from_spec() -> Self {
        // Keep signatures, overloads and edge policies tied to the normative tables.
        let section = LANGUAGE
            .split_once("## 15. Standard library\n")
            .expect("standard-library section")
            .1
            .split_once("## 16.")
            .expect("end of standard-library section")
            .0;
        let mut sections = section.split("### ");
        let common = sections.next().unwrap().trim();
        let mut catalog = Self {
            common,
            groups: Vec::new(),
            builtins: BTreeMap::new(),
        };
        for section in sections {
            let (name, body) = section.split_once('\n').unwrap();
            let mut names = Vec::new();
            let mut notes_start = 0;
            let mut offset = 0;
            for line in body.split_inclusive('\n') {
                if let Some(row) = line.strip_prefix("| `") {
                    let (signature, description) =
                        row.split_once(" | ").expect("builtin table row");
                    let name = signature.split_once('(').unwrap().0.to_owned();
                    let signatures = format!("`{signature}")
                        .replace('`', "")
                        .replace("\\|", "|")
                        .replace("; ", "\n");
                    names.push(name.clone());
                    catalog.builtins.insert(
                        name,
                        BuiltinDoc {
                            signatures,
                            description: description.trim().trim_end_matches('|').trim().to_owned(),
                            group: catalog.groups.len(),
                        },
                    );
                    notes_start = offset + line.len();
                }
                offset += line.len();
            }
            names.sort();
            catalog.groups.push(Group {
                name: name.trim(),
                names,
                notes: body[notes_start..].trim(),
            });
        }
        catalog
    }
}

#[derive(Default)]
struct Output {
    text: String,
    truncated: bool,
}

impl Write for Output {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !self.truncated {
            let mut end = text.len().min(OUTPUT_LIMIT - self.text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.push_str(&text[..end]);
            self.truncated = end < text.len();
        }
        Ok(())
    }
}

impl Output {
    fn finish(mut self) -> String {
        if self.truncated {
            let mut end = self.text.len().min(OUTPUT_LIMIT - OUTPUT_MARKER.len());
            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.truncate(end);
            self.text.push_str(OUTPUT_MARKER);
        }
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        registry::ToolDefinition,
        types::{Field, RecordType, derive_server},
    };
    use serde_json::json;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: Some(description.into()),
            input_schema: json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            }),
            output_schema: Some(json!({"type": "string"})),
            annotations: None,
        }
    }

    fn snapshot(tools: Vec<ToolDefinition>) -> ServerSnapshot {
        ServerSnapshot {
            status: ServerStatus::Ready,
            tools: tools
                .into_iter()
                .map(|tool| (tool.name.clone(), tool))
                .collect(),
            error: None,
        }
    }

    fn environment(servers: &BTreeMap<String, ServerSnapshot>) -> Environment {
        let mut environment = Environment::default();
        for (name, server) in servers {
            environment
                .add_server(name, derive_server(name, server.tools.values()))
                .unwrap();
        }
        environment
    }

    #[test]
    fn server_list_is_sorted_and_reports_unavailability_errors() {
        let mut unavailable = snapshot(vec![]);
        unavailable.status = ServerStatus::Unavailable;
        unavailable.error = Some("could not connect: missing executable".into());
        let servers = BTreeMap::from([
            ("zeta".into(), snapshot(vec![])),
            ("alpha".into(), unavailable),
        ]);
        let text = render("", &servers, &environment(&servers)).unwrap();
        assert!(text.starts_with("Servers\n  alpha: unavailable — could not connect: missing executable\n  zeta: available\n"));
    }

    #[test]
    fn server_summaries_and_full_descriptions_use_distinct_renderings() {
        let description =
            " First line\n\n  a very long second line: ".to_owned() + &"λ".repeat(1000) + "\n";
        let servers = BTreeMap::from([(
            "server".into(),
            snapshot(vec![
                tool("z-tool", &description),
                tool("a-tool", "First tool"),
            ]),
        )]);
        let environment = environment(&servers);
        let summary = render("server", &servers, &environment).unwrap();
        assert!(summary.starts_with("server: available\nserver.a-tool:"));
        assert!(
            summary.contains("server.z-tool: (args: {value: string}) => string\n  First line\n")
        );
        assert!(!summary.contains("second line"));
        let detail = render("server.z-tool", &servers, &environment).unwrap();
        assert_eq!(
            detail,
            format!("server.z-tool: (args: {{value: string}}) => string\n{description}")
        );
    }

    #[test]
    fn worked_example_help_is_exactly_signature_and_raw_description() {
        let mut navigate = tool("browser_navigate", "Navigate to a URL.");
        navigate.input_schema = json!({
            "type": "object",
            "properties": {"url": {"type": "string"}},
            "required": ["url"],
            "additionalProperties": false
        });
        navigate.output_schema = None;
        let servers = BTreeMap::from([("playwright".into(), snapshot(vec![navigate]))]);
        let text = render(
            "playwright.browser_navigate",
            &servers,
            &environment(&servers),
        )
        .unwrap();
        assert_eq!(
            text,
            "playwright.browser_navigate: (args: {url: string}) => Content[]\nNavigate to a URL."
        );
    }

    #[test]
    fn exact_matches_then_longest_server_prefix_win() {
        let servers = BTreeMap::from([
            (
                "s".into(),
                snapshot(vec![
                    tool("tool..name", "unusual tool"),
                    tool("deep.call", "short root"),
                    tool("deep", "not the server"),
                ]),
            ),
            ("s.deep".into(), snapshot(vec![tool("call", "long root")])),
        ]);
        let environment = environment(&servers);
        assert!(
            render("s.tool..name", &servers, &environment)
                .unwrap()
                .ends_with("unusual tool")
        );
        let detail = render("s.deep.call", &servers, &environment).unwrap();
        assert!(detail.ends_with("long root"));
        assert!(!detail.contains("short root"));
        assert!(
            render("s.deep", &servers, &environment)
                .unwrap()
                .starts_with("s.deep: available\n")
        );
        assert!(render("s.deep.missing", &servers, &environment).is_err());
        assert!(
            render("s.dee.call", &servers, &environment)
                .unwrap_err()
                .hint
                .unwrap()
                .contains("s.deep.call")
        );
    }

    #[test]
    fn reachable_recursive_definitions_print_once_without_unrelated_types() {
        let mut recursive = tool("tree", "Trees");
        recursive.output_schema = Some(json!({
            "$ref": "#/$defs/Node",
            "$defs": {"Node": {
                "type": "object",
                "properties": {
                    "value": {"type": "string"},
                    "children": {"type": "array", "items": {"$ref": "#/$defs/Node"}}
                },
                "required": ["value"],
                "additionalProperties": false
            }}
        }));
        let servers = BTreeMap::from([("s".into(), snapshot(vec![recursive]))]);
        let mut environment = environment(&servers);
        environment.definitions.insert(
            "Unrelated".into(),
            Type::Record(RecordType {
                fields: vec![Field {
                    name: "ignored".into(),
                    ty: Type::Named("Unrelated".into()),
                    optional: true,
                }],
                additional: None,
            }),
        );
        let text = render("s.tree", &servers, &environment).unwrap();
        assert_eq!(text.matches("Node = ").count(), 1, "{text}");
        assert!(text.contains("children?: Node[]"), "{text}");
        assert!(!text.contains("Unrelated"));
        assert!(!text.contains("Content ="));
    }

    #[test]
    fn output_cap_is_byte_based_and_unicode_safe() {
        for character in ["a", "é", "🐈"] {
            let servers = BTreeMap::from([(
                "s".into(),
                snapshot(vec![tool("large", &character.repeat(OUTPUT_LIMIT))]),
            )]);
            let text = render("s.large", &servers, &environment(&servers)).unwrap();
            assert!(text.len() <= OUTPUT_LIMIT);
            assert!(text.len() > OUTPUT_LIMIT - 4);
            assert!(text.ends_with(OUTPUT_MARKER));
        }
        let mut output = Output::default();
        output.write_str(&"x".repeat(OUTPUT_LIMIT)).unwrap();
        assert_eq!(output.finish().len(), OUTPUT_LIMIT);
    }
}
