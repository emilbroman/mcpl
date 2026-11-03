use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "RawConfig")]
pub struct Config {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, try_from = "RawServerConfig")]
pub enum ServerConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
    },
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        Self::parse(&source).with_context(|| format!("failed to parse config {}", path.display()))
    }

    pub fn parse(source: &str) -> Result<Self> {
        serde_json::from_str(source).context("invalid MCP server configuration")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<String, ServerConfig>,
}

impl TryFrom<RawConfig> for Config {
    type Error = String;

    fn try_from(raw: RawConfig) -> std::result::Result<Self, Self::Error> {
        if raw.mcp_servers.keys().any(|name| name.trim().is_empty()) {
            return Err("server names must not be empty".into());
        }
        Ok(Self {
            mcp_servers: raw.mcp_servers,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServerConfig {
    #[serde(default, deserialize_with = "present")]
    command: Option<String>,
    #[serde(default, deserialize_with = "present")]
    args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    env: Option<BTreeMap<String, String>>,
    #[serde(default, deserialize_with = "present")]
    url: Option<String>,
}

// Missing fields are optional, but explicit null is not a valid field value.
fn present<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl TryFrom<RawServerConfig> for ServerConfig {
    type Error = String;

    fn try_from(raw: RawServerConfig) -> std::result::Result<Self, Self::Error> {
        match (raw.command, raw.url) {
            (Some(_), Some(_)) => Err("server must not specify both command and url".into()),
            (Some(command), None) => {
                if command.trim().is_empty() {
                    return Err("server command must not be empty".into());
                }
                Ok(Self::Stdio {
                    command,
                    args: raw.args.unwrap_or_default(),
                    env: raw.env.unwrap_or_default(),
                })
            }
            (None, Some(url)) => {
                if url.trim().is_empty() {
                    return Err("server url must not be empty".into());
                }
                if raw.args.is_some() || raw.env.is_some() {
                    return Err("HTTP servers do not support args or env".into());
                }
                Ok(Self::Http { url })
            }
            (None, None) => Err("server must specify command or url".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stdio_and_http_servers() {
        let config = Config::parse(
            r#"{"mcpServers": {
                "local": {"command": "npx", "args": ["@playwright/mcp"], "env": {"MODE": "test"}},
                "remote": {"url": "https://example.com/mcp"}
            }}"#,
        )
        .unwrap();
        match &config.mcp_servers["local"] {
            ServerConfig::Stdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args, &["@playwright/mcp"]);
                assert_eq!(env["MODE"], "test");
            }
            _ => panic!("expected stdio server"),
        }
        assert!(matches!(
            &config.mcp_servers["remote"],
            ServerConfig::Http { url } if url == "https://example.com/mcp"
        ));
    }

    #[test]
    fn defaults_stdio_args_and_env() {
        let config = Config::parse(r#"{"mcpServers":{"local":{"command":"server"}}}"#).unwrap();
        assert!(matches!(
            &config.mcp_servers["local"],
            ServerConfig::Stdio { args, env, .. } if args.is_empty() && env.is_empty()
        ));
    }

    #[test]
    fn requires_mcp_servers_and_valid_json() {
        for source in ["{}", "not json", r#"{"mcpServers":[]}"#] {
            assert!(Config::parse(source).is_err(), "accepted {source}");
        }
        assert!(Config::parse(r#"{"mcpServers":{}}"#).is_ok());
    }

    #[test]
    fn rejects_invalid_servers() {
        for server in [
            r#"{}"#,
            r#"{"command":""}"#,
            r#"{"command":"  "}"#,
            r#"{"url":""}"#,
            r#"{"url":"  "}"#,
            r#"{"command":"server","url":"https://example.com"}"#,
            r#"{"command":123}"#,
            r#"{"url":123}"#,
            r#"{"command":null}"#,
            r#"{"command":"server","args":null}"#,
            r#"{"command":"server","args":[123]}"#,
            r#"{"command":"server","env":{"PORT":123}}"#,
            r#"{"command":"server","env":null}"#,
            r#"{"url":"https://example.com","args":[]}"#,
            r#"{"url":"https://example.com","env":{}}"#,
            r#"{"url":"https://example.com","headers":{}}"#,
            r#"{"command":"server","transport":"sse"}"#,
        ] {
            let source = format!(r#"{{"mcpServers":{{"test":{server}}}}}"#);
            assert!(Config::parse(&source).is_err(), "accepted {server}");
        }
    }

    #[test]
    fn rejects_empty_server_names() {
        for name in ["", "   "] {
            let source = format!(r#"{{"mcpServers":{{"{name}":{{"command":"server"}}}}}}"#);
            assert!(Config::parse(&source).is_err());
        }
    }

    #[test]
    fn reports_actionable_errors() {
        let error = Config::parse(
            r#"{"mcpServers":{"test":{"command":"server","url":"https://example.com"}}}"#,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("both command and url"));
        let error = Config::parse(
            r#"{"mcpServers":{"test":{"url":"https://example.com","transport":"sse"}}}"#,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("unknown field `transport`"));
    }

    #[test]
    fn load_errors_include_path() {
        let path =
            std::env::temp_dir().join(format!("mcpl-missing-config-{}.json", std::process::id()));
        let error = Config::load(&path).unwrap_err();
        assert!(error.to_string().contains(&path.display().to_string()));
    }
}
