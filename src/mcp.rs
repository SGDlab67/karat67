//! Minimal stdio JSON-RPC server so an agent can call the account gate.
//!
//! Speaks `initialize`, `tools/list`, and `tools/call`. The only tool is
//! `check_account`, with the same inputs as `karat account`. The tool text is
//! JSON `{ "ok": bool, "results": [...] }`. `ok` is true only when every
//! result is Pass.
//!
//! Input is one JSON object per line, or a `Content-Length` frame. Replies are
//! `Content-Length` frames. No MCP SDK.

use std::io::{BufRead, Write};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::checks::reconcile::DEFAULT_MAX_SLOT_LAG;
use crate::fetch::{AccountFetcher, RpcAccountFetcher};
use crate::gate;

/// Read JSON-RPC messages from `input` and write responses to `output`.
///
/// Returns when `input` reaches EOF. Notifications (no `id`) produce no reply.
pub fn serve(input: &mut impl BufRead, output: &mut impl Write) -> anyhow::Result<()> {
    loop {
        let Some(message) = read_message(input)? else {
            return Ok(());
        };
        if let Some(response) = handle_request(&message) {
            write_message(output, &response)?;
        }
    }
}

fn handle_request(request: &Value) -> Option<Value> {
    let id = request.get("id")?;
    if id.is_null() {
        return None;
    }
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").unwrap_or(&Value::Null);
    let outcome = match method {
        "initialize" => Ok(initialize_result(params)),
        "tools/list" => Ok(tools_list()),
        "ping" => Ok(json!({})),
        "tools/call" => tools_call(params),
        _ => Err(error_object(-32601, format!("method not found: {method}"))),
    };
    Some(match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    })
}

fn initialize_result(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "karat-mcp", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [{
            "name": "check_account",
            "description": "Shape-check indexed account bytes, then reconcile against RPC only if shape passes. ok is true only when every result is Pass. A shape failure does not call RPC.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "account": { "type": "string", "description": "Account pubkey" },
                    "indexed_base64": { "type": "string", "description": "Base64 indexed account bytes" },
                    "indexed_slot": { "type": "integer", "description": "Indexer write slot. Omit for strict mismatch = Fail" },
                    "max_slot_lag": { "type": "integer", "description": "Slot lag tolerated on a mismatch before Fail. Default 32" },
                    "rpc_url": { "type": "string", "description": "Solana JSON-RPC URL. Falls back to KARAT_RPC_URL. Unused when shape fails" }
                },
                "required": ["account", "indexed_base64"]
            }
        }]
    })
}

fn tools_call(params: &Value) -> Result<Value, Value> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != "check_account" {
        return Err(error_object(-32602, format!("unknown tool {name}")));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(error_object(-32602, "arguments must be an object"));
    }
    let payload = run_check_account(&arguments).map_err(|message| error_object(-32602, message))?;
    let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let text = serde_json::to_string(&payload)
        .map_err(|e| error_object(-32603, format!("encoding result failed: {e}")))?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": !ok,
    }))
}

fn run_check_account(args: &Value) -> Result<Value, String> {
    let account = required_str(args, "account")?;
    let encoded = required_str(args, "indexed_base64")?;
    let indexed = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("invalid base64: {e}"))?;
    let indexed_slot = optional_u64(args, "indexed_slot")?;
    let max_slot_lag = optional_u64(args, "max_slot_lag")?.unwrap_or(DEFAULT_MAX_SLOT_LAG);
    let fetcher = rpc_url_from(args).map(RpcAccountFetcher::new);
    let results = gate::check_account(
        account,
        &indexed,
        indexed_slot,
        max_slot_lag,
        fetcher.as_ref().map(|fetch| fetch as &dyn AccountFetcher),
    );
    Ok(json!({
        "ok": gate::all_pass(&results),
        "results": results,
    }))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key).and_then(Value::as_str) {
        Some(value) if key == "indexed_base64" || !value.is_empty() => Ok(value),
        _ => Err(format!("missing {key}")),
    }
}

fn optional_u64(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be an integer")),
    }
}

fn rpc_url_from(args: &Value) -> Option<String> {
    let from_args = args
        .get("rpc_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if from_args.is_some() {
        return from_args;
    }
    std::env::var("KARAT_RPC_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn error_object(code: i64, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn read_message(input: &mut impl BufRead) -> anyhow::Result<Option<Value>> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') {
            let value = serde_json::from_str(trimmed)
                .map_err(|e| anyhow::anyhow!("invalid JSON-RPC message: {e}"))?;
            return Ok(Some(value));
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(len_str) = lower.strip_prefix("content-length:") {
            let len: usize = len_str
                .trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid Content-Length: {e}"))?;
            loop {
                line.clear();
                if input.read_line(&mut line)? == 0 {
                    anyhow::bail!("unexpected eof in MCP headers");
                }
                if line.trim().is_empty() {
                    break;
                }
            }
            let mut buf = vec![0u8; len];
            input.read_exact(&mut buf)?;
            let value = serde_json::from_slice(&buf)
                .map_err(|e| anyhow::anyhow!("invalid JSON-RPC body: {e}"))?;
            return Ok(Some(value));
        }
        anyhow::bail!("expected a JSON-RPC object or Content-Length header, got {trimmed:?}");
    }
}

fn write_message(output: &mut impl Write, value: &Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frames(buf: &[u8]) -> Vec<Value> {
        let mut rest = std::str::from_utf8(buf).expect("utf8").to_string();
        let mut out = Vec::new();
        while !rest.trim().is_empty() {
            let (header, after) = rest.split_once("\r\n\r\n").expect("header frame");
            let len: usize = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_string)
                })
                .expect("content-length")
                .parse()
                .expect("length");
            let body = after[..len].to_string();
            out.push(serde_json::from_str(&body).expect("json body"));
            rest = after[len..].to_string();
        }
        out
    }

    #[test]
    fn initialize_lists_check_account_and_ignores_notifications() {
        let init = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        }))
        .expect("response");
        assert_eq!(init["result"]["serverInfo"]["name"], "karat-mcp");
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");

        let list = handle_request(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))
        .expect("response");
        assert_eq!(list["result"]["tools"][0]["name"], "check_account");
        assert!(
            handle_request(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .is_none()
        );
    }

    #[test]
    fn stdin_tools_call_empty_bytes_is_not_ok() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":",
            "{\"name\":\"check_account\",\"arguments\":{\"account\":\"acct\",\"indexed_base64\":\"\"}}}\n",
        );
        let mut output = Vec::new();
        serve(&mut Cursor::new(input), &mut output).expect("serve");
        let messages = frames(&output);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["id"], 1);
        assert_eq!(messages[0]["result"]["isError"], true);

        let text = messages[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        let payload: Value = serde_json::from_str(text).expect("payload");
        assert_eq!(payload["ok"], false);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["status"], "Fail");
        assert!(results[0]["check"].as_str().unwrap().starts_with("shape"));
    }

    #[test]
    fn content_length_tools_list() {
        let body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut output = Vec::new();
        serve(&mut Cursor::new(framed.into_bytes()), &mut output).expect("serve");
        let messages = frames(&output);
        assert_eq!(messages[0]["result"]["tools"][0]["name"], "check_account");
    }
}
