#!/usr/bin/env python3
"""Capture public tool discovery without invoking tools or launching a browser."""

import json
import os
from pathlib import Path
import signal
import subprocess


fixture = Path(__file__).with_name("playwright-tools.json")
env = dict(os.environ)
env.setdefault("npm_config_cache", "/workspace/.npm-cache")
process = subprocess.Popen(
    ["npx", "--yes", "@playwright/mcp@0.0.84", "--headless"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
    env=env,
    start_new_session=True,
)


def send(message):
    process.stdin.write(json.dumps(message) + "\n")
    process.stdin.flush()


def response(request_id):
    for line in process.stdout:
        message = json.loads(line)
        if message.get("id") == request_id:
            if "error" in message:
                raise RuntimeError(message["error"])
            return message["result"]
    raise RuntimeError(f"Server exited before response {request_id}")


try:
    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "mcpl-fixture-capture", "version": "1.0.0"},
        },
    })
    initialization = response(1)
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
    result = response(2)
    if result.get("nextCursor"):
        raise RuntimeError("Unexpected pagination; refusing to save a partial tool index")
    fixture.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({
        "serverInfo": initialization["serverInfo"],
        "protocolVersion": initialization["protocolVersion"],
        "toolCount": len(result["tools"]),
        "fixture": str(fixture),
    }, indent=2))
finally:
    process.stdin.close()
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
    process.wait()
