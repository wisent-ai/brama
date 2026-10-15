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

use serde::Deserialize;
use serde_json::{json, Value};

use crate::detect_compute_resources;

/// The MCP revision this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";
/// JSON-RPC 2.0's reserved error codes this server answers with.
const CODE_PARSE_ERROR: i64 = -32700; // https://www.jsonrpc.org/specification#error_object "-32700 Parse error"
const CODE_INVALID_REQUEST: i64 = -32600; // https://www.jsonrpc.org/specification#error_object "-32600 Invalid Request"
const CODE_METHOD_NOT_FOUND: i64 = -32601; // https://www.jsonrpc.org/specification#error_object "-32601 Method not found"
const CODE_INVALID_PARAMS: i64 = -32602; // https://www.jsonrpc.org/specification#error_object "-32602 Invalid params"
/// The implementation-defined server error a tool that refused answers with.
const CODE_SERVER_ERROR: i64 = -32000; // https://www.jsonrpc.org/specification#error_object "-32000 to -32099 Server error"

/// One JSON-RPC message as the client sent it. A message without `id` is a
/// notification and is never answered.
#[derive(Deserialize)]
struct Request {
    method: String,
    id: Option<Value>,
    params: Option<Value>,
}

/// The parameters of `tools/call`: the tool's name, and its arguments when
/// the tool takes any.
#[derive(Deserialize)]
struct ToolCall {
    name: String,
}

fn schema(properties: Value, required: Vec<&str>) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

// Read-only, credential-free allow-list.
fn tools() -> Value {
    json!([
        {"name": "brama_detect",
         "description": "Detect local compute resources: GPU type and name, dedicated VRAM (null where the GPU shares system memory or none was reported), RAM, CPU cores, CUDA/Metal. Local only; no network, no credentials, no cost.",
         "inputSchema": schema(json!({}), vec![])},
    ])
}

fn text_result(value: &Value) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        other => format!("{other:#}"),
    };
    json!({"content": [{"type": "text", "text": text}]})
}

fn detect_tool() -> Value {
    let res = detect_compute_resources();
    json!({
        "gpu_type": res.gpu_type,
        "gpu_name": res.gpu_name,
        "vram_gb": res.vram_gb,
        "ram_gb": res.ram_gb,
        "cpu_cores": res.cpu_cores,
        "has_cuda": res.has_cuda,
        "has_metal": res.has_metal,
    })
}

fn call_tool(name: &str) -> Result<Value, String> {
    match name {
        "brama_detect" => Ok(text_result(&detect_tool())),
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Write one frame; a client that stopped reading is the caller's error.
fn send(out: &mut impl Write, message: &Value) -> std::io::Result<()> {
    writeln!(out, "{message}")?;
    out.flush()
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The answer to one request, or `None` for a notification.
fn answer(request: Request) -> Option<Value> {
    let id = request.id?;
    let reply = match request.method.as_str() {
        "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "brama", "version": env!("CARGO_PKG_VERSION")}}}),
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}),
        "tools/call" => {
            let call = match request.params {
                Some(params) => serde_json::from_value::<ToolCall>(params)
                    .map_err(|error| format!("tools/call params: {error}")),
                None => Err("tools/call needs params naming the tool".to_string()),
            };
            match call {
                Ok(call) => match call_tool(&call.name) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err(refusal) => error_response(id, CODE_SERVER_ERROR, &refusal),
                },
                Err(refusal) => error_response(id, CODE_INVALID_PARAMS, &refusal),
            }
        }
        other => error_response(
            id,
            CODE_METHOD_NOT_FOUND,
            &format!("method not found: {other}"),
        ),
    };
    Some(reply)
}

/// Run the stdio JSON-RPC loop until stdin closes. Owns stdout exclusively.
/// Reading stdin or writing stdout failing ends the loop with that reason.
pub fn serve() -> Result<(), String> {
    eprintln!("brama MCP server on stdio (protocol {PROTOCOL_VERSION})");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("stdin cannot be read: {error}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(trimmed) {
            Ok(message) => match serde_json::from_value::<Request>(message) {
                Ok(request) => answer(request),
                Err(error) => Some(error_response(
                    Value::Null,
                    CODE_INVALID_REQUEST,
                    &format!("invalid request: {error}"),
                )),
            },
            Err(error) => Some(error_response(
                Value::Null,
                CODE_PARSE_ERROR,
                &format!("parse error: {error}"),
            )),
        };
        if let Some(reply) = reply {
            send(&mut stdout, &reply)
                .map_err(|error| format!("stdout cannot be written: {error}"))?;
        }
    }
    Ok(())
}
