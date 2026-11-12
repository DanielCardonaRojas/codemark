//! A minimal stdio MCP server: newline-delimited JSON-RPC 2.0 with
//! `initialize`, `ping`, `tools/list` and `tools/call`, plus outgoing
//! notifications. Hand-rolled on purpose: it is the small subset a channel
//! needs, and it adds no dependencies (see the openssl-free TLS rule).

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Protocol version answered when the client does not name one.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// What a server built on [`serve`] provides.
pub trait Handler {
    /// `capabilities` for the `initialize` result.
    fn capabilities(&self) -> Value;
    /// `instructions` for the `initialize` result; the client hands it to
    /// the model as context.
    fn instructions(&self) -> Option<String>;
    fn server_name(&self) -> &str;
    /// Tool descriptors for `tools/list`.
    fn tools(&self) -> Vec<Value>;
    /// Run tool `name`. `Ok` text is the tool's output; `Err` text is
    /// returned as a tool error the model can read and retry.
    fn call_tool(&self, name: &str, args: &Value) -> Result<String, String>;
}

/// The write half of the transport. Cloneable so a background thread can
/// send notifications while the request loop answers requests; each message
/// is written as one line under the lock.
#[derive(Clone)]
pub struct Outbound {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Outbound {
    pub fn new(w: impl Write + Send + 'static) -> Self {
        Self { inner: Arc::new(Mutex::new(Box::new(w))) }
    }

    pub fn send(&self, msg: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        let mut w = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(line.as_bytes())?;
        w.flush()
    }

    pub fn notify(&self, method: &str, params: Value) -> std::io::Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }
}

/// Answer requests read from `input` until it closes.
pub fn serve(input: impl BufRead, out: &Outbound, handler: &impl Handler) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                out.send(&error(Value::Null, -32700, &format!("parse error: {e}")))?;
                continue;
            }
        };
        if let Some(reply) = handle_message(&msg, handler) {
            out.send(&reply)?;
        }
    }
    Ok(())
}

/// The response to one incoming message; `None` for notifications and for
/// responses to requests we never send.
pub fn handle_message(msg: &Value, handler: &impl Handler) -> Option<Value> {
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    tracing::debug!(target: "codemark::agent", method, "mcp request");
    Some(match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION);
            let mut result = json!({
                "protocolVersion": version,
                "capabilities": handler.capabilities(),
                "serverInfo": {"name": handler.server_name(), "version": env!("CARGO_PKG_VERSION")},
            });
            if let Some(i) = handler.instructions() {
                result["instructions"] = Value::String(i);
            }
            ok(id, result)
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({"tools": handler.tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let (text, is_error) = match handler.call_tool(name, &args) {
                Ok(t) => (t, false),
                Err(t) => (t, true),
            };
            ok(id, json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        _ => error(id, -32601, &format!("method not found: {method}")),
    })
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// A required string argument.
pub fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing required argument `{key}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    impl Handler for Echo {
        fn capabilities(&self) -> Value {
            json!({"tools": {}})
        }
        fn instructions(&self) -> Option<String> {
            Some("hi".into())
        }
        fn server_name(&self) -> &str {
            "echo"
        }
        fn tools(&self) -> Vec<Value> {
            vec![json!({"name": "echo"})]
        }
        fn call_tool(&self, name: &str, args: &Value) -> Result<String, String> {
            match name {
                "echo" => arg_str(args, "text").map(str::to_string),
                _ => Err(format!("unknown tool {name}")),
            }
        }
    }

    #[test]
    fn initialize_echoes_version_and_instructions() {
        let r = handle_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
            &Echo,
        )
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["instructions"], "hi");
        assert_eq!(r["result"]["serverInfo"]["name"], "echo");
    }

    #[test]
    fn notifications_get_no_answer() {
        assert!(
            handle_message(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), &Echo)
                .is_none()
        );
    }

    #[test]
    fn tool_errors_are_tool_results() {
        let r = handle_message(
            &json!({"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"name":"echo","arguments":{}}}),
            &Echo,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        let r = handle_message(
            &json!({"jsonrpc":"2.0","id":"b","method":"tools/call","params":{"name":"echo","arguments":{"text":"x"}}}),
            &Echo,
        )
        .unwrap();
        assert_eq!(r["result"]["content"][0]["text"], "x");
    }

    #[test]
    fn unknown_method_is_an_error() {
        let r = handle_message(&json!({"jsonrpc":"2.0","id":2,"method":"nope"}), &Echo).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }
}
