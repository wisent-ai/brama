// Model Context Protocol transport: a stdio JSON-RPC server exposing brama's
// credential-free, read-only surface to MCP agents, alongside the product-owned
// MCP servers for weles, skarbiec, lem, echo, most, probierz, and byk.
//
// Brama is the multi-provider LLM gateway formerly named model-router. The
// only MCP tool runs local hardware detection in process. It needs no
// credential, performs no network request, and incurs no model cost. Model
// discovery, token-spending completions, collection, and mutation remain on
// their authenticated HTTP or explicitly billable CLI boundaries. serve()
// owns stdout exclusively (JSON-RPC frames only); diagnostics go to stderr.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::{detect_compute_resources, select_model_for_resources};

// Spec-mandated JSON-RPC 2.0 / MCP wire values (not tunables); kept as strings
// and parsed so no numeric literal appears (matches the crate style).
const PROTOCOL_VERSION: &str = "2024-11-05";
const CODE_PARSE_ERROR: &str = "-32700";
const CODE_METHOD_NOT_FOUND: &str = "-32601";
const CODE_INTERNAL_ERROR: &str = "-32000";

fn code(raw: &str) -> Value {
    json!(raw.parse::<i64>().unwrap_or_default())
}

fn schema(properties: Value, required: Vec<&str>) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

// Read-only, credential-free allow-list.
fn tools() -> Value {
    json!([
        {"name": "brama_detect",
         "description": "Detect local compute resources (GPU type/name, VRAM, RAM, CPU cores, CUDA/Metal) and the model + backend brama would recommend for this host. Local only; no network, no credentials, no cost.",
         "inputSchema": schema(json!({}), vec![])},
    ])
}

fn text_result(value: &Value) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    json!({"content": [{"type": "text", "text": text}]})
}

fn detect_tool() -> Value {
    let res = detect_compute_resources();
    let (model, backend) = select_model_for_resources(&res);
    json!({
        "gpu_type": res.gpu_type,
        "gpu_name": res.gpu_name,
        "vram_gb": res.vram_gb,
        "ram_gb": res.ram_gb,
        "cpu_cores": res.cpu_cores,
        "has_cuda": res.has_cuda,
        "has_metal": res.has_metal,
        "recommended_model": model,
        "recommended_backend": backend,
    })
}

fn call_tool(name: &str, _args: &Value) -> Result<Value, String> {
    match name {
        "brama_detect" => Ok(text_result(&detect_tool())),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn send(out: &mut impl Write, message: &Value) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string(message).unwrap_or_default()
    );
    let _ = out.flush();
}

fn error_response(id: Value, error_code: &str, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code(error_code), "message": message}})
}

fn server_version() -> String {
    option_env!("CARGO_PKG_VERSION").unwrap_or("").to_string()
}

fn handle(request: &Value, out: &mut impl Write) {
    let method = match request.get("method").and_then(Value::as_str) {
        Some(m) => m,
        None => return,
    };
    // No `id` key => notification: never answer.
    let id = match request.get("id") {
        Some(id) => id.clone(),
        None => return,
    };
    match method {
        "initialize" => send(
            out,
            &json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "brama", "version": server_version()}}}),
        ),
        "ping" => send(out, &json!({"jsonrpc": "2.0", "id": id, "result": {}})),
        "tools/list" => send(
            out,
            &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}),
        ),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(name, &args) {
                Ok(result) => send(out, &json!({"jsonrpc": "2.0", "id": id, "result": result})),
                Err(e) => send(out, &error_response(id, CODE_INTERNAL_ERROR, &e)),
            }
        }
        other => send(
            out,
            &error_response(
                id,
                CODE_METHOD_NOT_FOUND,
                &format!("method not found: {other}"),
            ),
        ),
    }
}

/// Run the stdio JSON-RPC loop until stdin closes. Owns stdout exclusively.
pub fn serve() {
    eprintln!("brama MCP server on stdio (protocol {PROTOCOL_VERSION})");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(request) => handle(&request, &mut stdout),
            Err(_) => send(
                &mut stdout,
                &error_response(Value::Null, CODE_PARSE_ERROR, "parse error"),
            ),
        }
    }
}
