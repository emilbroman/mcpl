use std::{process::Stdio, time::Duration};

use anyhow::{Context, bail};
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
    let mut client = tokio::time::timeout(Duration::from_secs(45), ().serve(transport))
        .await
        .context("proxy startup timed out")??;
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let tools = client.list_all_tools().await?;
        println!("tools/list: {}", serde_json::to_string_pretty(&tools)?);
        anyhow::ensure!(
            tools.len() == 1 && tools[0].name == "run",
            "expected only the run tool"
        );
        let result = client
            .call_tool(
                CallToolRequestParams::new("run")
                    .with_arguments(json!({"program": "inspect"}).as_object().unwrap().clone()),
            )
            .await?;
        println!("run: {}", serde_json::to_string_pretty(&result)?);
        anyhow::Ok(())
    })
    .await
    .context("inspection timed out")?;
    client.close_with_timeout(Duration::from_secs(10)).await?;
    result
}
