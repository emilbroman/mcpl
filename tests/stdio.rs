use std::{process::Stdio, time::Duration};

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::json;

#[tokio::test]
async fn binary_stays_up_with_failed_downstreams_and_clean_stdout() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let config_path =
            std::env::temp_dir().join(format!("mcpl-stdio-{}.json", std::process::id()));
        let config = json!({"mcpServers": {
            "missing-server": {"command": "/nonexistent/mcpl-test-server"}
        }});
        std::fs::write(&config_path, serde_json::to_vec(&config)?)?;
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcpl"));
        command.arg("--config").arg(&config_path);
        let (transport, _) = TokioChildProcess::builder(command)
            .stderr(Stdio::inherit())
            .spawn()?;
        let connected = ().serve(transport).await;
        std::fs::remove_file(config_path)?;
        let mut client = connected?;
        let tools = client.list_all_tools().await?;
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "run");
        let result = client
            .call_tool(
                CallToolRequestParams::new("help")
                    .with_arguments(json!({}).as_object().unwrap().clone()),
            )
            .await?;
        let value = result.content[0].as_text().unwrap().text.as_str();
        assert!(value.contains("missing-server"));
        assert!(value.contains("unavailable"));
        assert!(value.contains("could not spawn"));
        assert_ne!(result.is_error, Some(true));
        client.close().await?;
        anyhow::Ok(())
    })
    .await?
}

#[test]
fn invalid_configuration_has_a_clear_error_and_no_stdout() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcpl"))
        .args(["--config", "/nonexistent/mcpl-test-config.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to read config"));
}
