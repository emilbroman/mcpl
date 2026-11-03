use std::process::Stdio;

use anyhow::bail;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        bail!("usage: cargo run --example inspect -- <mcpl-binary> <config.json>");
    }
    let mut command = tokio::process::Command::new(&args[0]);
    command.args(["--config", &args[1]]);
    let (transport, _) = TokioChildProcess::builder(command)
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut client = ().serve(transport).await?;
    let result = async {
        let tools = client.list_all_tools().await?;
        println!("tools/list: {}", serde_json::to_string_pretty(&tools)?);
        anyhow::ensure!(
            tools.len() == 3
                && ["run", "help", "show"]
                    .iter()
                    .all(|name| tools.iter().any(|tool| tool.name == *name)),
            "expected the run, help and show tools"
        );
        let result = client
            .call_tool(
                CallToolRequestParams::new("help")
                    .with_arguments(json!({}).as_object().unwrap().clone()),
            )
            .await?;
        println!("help: {}", serde_json::to_string_pretty(&result)?);
        anyhow::Ok(())
    }
    .await;
    client.close().await?;
    result
}
