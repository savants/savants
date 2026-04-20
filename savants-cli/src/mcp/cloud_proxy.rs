//! Cloud proxy MCP server: forwards all tool calls to api.savants.cloud
//! instead of querying a local FalkorDB instance.
//!
//! When SAVANTS_CLOUD_URL is set, `savants serve` uses this instead of
//! the local McpServer. The developer doesn't need FalkorDB installed.

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

pub struct CloudProxyServer {
    cloud_url: String,
    api_key: String,
}

impl CloudProxyServer {
    pub fn new(cloud_url: &str, api_key: &str) -> Self {
        Self {
            cloud_url: cloud_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    pub fn run(&self) {
        eprintln!("Savants MCP server started (cloud proxy -> {})", self.cloud_url);
        let stdin = io::stdin();
        let stdout = io::stdout();
        let reader = stdin.lock();
        let mut writer = stdout.lock();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }

            let message: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(response) = self.handle_message(&message) {
                let body = serde_json::to_string(&response).unwrap();
                let _ = writeln!(writer, "{}", body);
                let _ = writer.flush();
            }
        }
    }

    fn handle_message(&self, message: &Value) -> Option<Value> {
        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let req_id = message.get("id");

        if req_id.is_none() || req_id == Some(&Value::Null) {
            return None;
        }
        let req_id = req_id.unwrap().clone();

        match method {
            "initialize" => Some(self.response(&req_id, json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {},
                    "prompts": {}
                },
                "serverInfo": {"name": "savants", "version": "0.1.0-cloud"}
            }))),

            "ping" => Some(self.response(&req_id, json!({}))),

            "tools/list" => {
                // Fetch tool list from cloud API
                match self.cloud_get("/api/v1/tools") {
                    Ok(tools_response) => {
                        let tools = tools_response.get("tools").cloned().unwrap_or(json!([]));
                        // Convert cloud format to MCP format
                        let mcp_tools: Vec<Value> = tools.as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .map(|t| {
                                json!({
                                    "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                    "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                                    "inputSchema": {
                                        "type": "object",
                                        "properties": {}
                                    }
                                })
                            })
                            .collect();
                        Some(self.response(&req_id, json!({"tools": mcp_tools})))
                    }
                    Err(e) => Some(self.error(&req_id, -32000, &format!("Cloud error: {}", e))),
                }
            }

            "tools/call" => {
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                // Forward to cloud API
                let body = json!({
                    "tool": tool_name,
                    "arguments": arguments,
                });

                match self.cloud_post("/api/v1/tools/call", &body) {
                    Ok(cloud_response) => {
                        let result_text = cloud_response.get("result")
                            .and_then(|v| v.as_str())
                            .unwrap_or_else(|| {
                                cloud_response.get("result")
                                    .map(|v| v.to_string())
                                    .unwrap_or_default()
                                    .leak()
                            });

                        Some(self.response(&req_id, json!({
                            "content": [{
                                "type": "text",
                                "text": result_text
                            }]
                        })))
                    }
                    Err(e) => Some(self.error(&req_id, -32000, &format!("Cloud error: {}", e))),
                }
            }

            "resources/list" => Some(self.response(&req_id, json!({"resources": []}))),
            "prompts/list" => Some(self.response(&req_id, json!({"prompts": []}))),
            _ => Some(self.error(&req_id, -32601, &format!("Unknown method: {}", method))),
        }
    }

    fn cloud_get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.cloud_url, path);
        let output = std::process::Command::new("curl")
            .args(["-sf", "--max-time", "60", "-H", &format!("Authorization: Bearer {}", self.api_key), &url])
            .output()
            .map_err(|e| format!("curl failed: {}", e))?;
        if !output.status.success() {
            return Err(format!("HTTP error from {}", url));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("parse failed: {}", e))
    }

    fn cloud_post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}{}", self.cloud_url, path);
        let body_str = serde_json::to_string(body).unwrap();
        let output = std::process::Command::new("curl")
            .args([
                "-sf", "--max-time", "60",
                "-X", "POST",
                "-H", &format!("Authorization: Bearer {}", self.api_key),
                "-H", "Content-Type: application/json",
                "-d", &body_str,
                &url,
            ])
            .output()
            .map_err(|e| format!("curl failed: {}", e))?;
        if !output.status.success() {
            return Err(format!("HTTP error from {}", url));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("parse failed: {}", e))
    }

    fn response(&self, id: &Value, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    fn error(&self, id: &Value, code: i32, message: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
    }
}
