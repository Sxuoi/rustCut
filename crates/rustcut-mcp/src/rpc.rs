//! A small, strict MCP (JSON-RPC 2.0 over stdio) server loop.
//!
//! stdout is the protocol channel: everything written there goes through [`Server::send`];
//! logs go to stderr and `<logs>/mcp.log`.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::env::Env;
use crate::policy::uri_to_path;
use crate::tools::{self, Ctx, Progress};

/// Protocol versions this server speaks, newest first.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const ROOTS_REQUEST_ID: &str = "rustcut-roots-1";

/// Shared server state.
pub struct Server {
    env: Arc<Env>,
    out: UnboundedSender<Value>,
    client_roots: AtomicBool,
    roots_pending: AtomicBool,
    running: Mutex<HashMap<String, tokio::task::AbortHandle>>,
}

impl Server {
    fn send(&self, message: Value) {
        let _ = self.out.send(message);
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn reply_error(&self, id: &Value, code: i64, message: &str) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }

    fn request_roots(&self) {
        if self.client_roots.load(Ordering::Relaxed) {
            self.roots_pending.store(true, Ordering::Relaxed);
            self.send(json!({"jsonrpc": "2.0", "id": ROOTS_REQUEST_ID, "method": "roots/list"}));
        }
    }

    async fn wait_for_roots(&self) {
        for _ in 0..120 {
            if !self.roots_pending.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.roots_pending.store(false, Ordering::Relaxed);
    }

    fn handle_response(&self, msg: &Value) {
        if msg["id"] != json!(ROOTS_REQUEST_ID) {
            return;
        }
        if let Some(roots) = msg["result"]["roots"].as_array() {
            let paths: Vec<_> = roots
                .iter()
                .filter_map(|r| r["uri"].as_str().and_then(uri_to_path))
                .collect();
            tracing::info!(count = paths.len(), "client roots received");
            self.env.policy.set_client_roots(paths);
        } else {
            tracing::warn!("roots/list failed; continuing with the environment roots only");
        }
        self.roots_pending.store(false, Ordering::Relaxed);
    }

    fn handle_initialize(&self, id: &Value, params: &Value) {
        let wanted = params["protocolVersion"].as_str().unwrap_or("");
        let version = SUPPORTED_VERSIONS
            .iter()
            .find(|v| **v == wanted)
            .copied()
            .unwrap_or(SUPPORTED_VERSIONS[0]);
        self.client_roots.store(
            params["capabilities"].get("roots").is_some(),
            Ordering::Relaxed,
        );
        self.reply(
            id,
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {
                    "name": "rustcut-mcp",
                    "title": "RustCut",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": tools::INSTRUCTIONS,
            }),
        );
    }

    fn handle_call(self: &Arc<Self>, id: Value, params: Value) {
        let key = id.to_string();
        let this = self.clone();
        let task_key = key.clone();
        let handle = tokio::spawn(async move {
            let name = params["name"].as_str().unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let progress = params["_meta"]
                .get("progressToken")
                .filter(|t| t.is_string() || t.is_number())
                .map(|token| Progress {
                    token: token.clone(),
                    tx: this.out.clone(),
                });
            this.wait_for_roots().await;
            let ctx = Ctx {
                env: this.env.clone(),
                progress,
            };
            tracing::info!(tool = %name, "tool call");
            let call_name = name.clone();
            let output = match tokio::spawn(async move { tools::call(ctx, &call_name, args).await })
                .await
            {
                Ok(out) => out,
                Err(e) => {
                    tracing::error!(tool = %name, error = %e, "tool task failed");
                    tools::ToolOutput::err(format!(
                        "Internal error while running {name}: {e}. Try again; if it persists, check the log file mcp.log."
                    ))
                }
            };
            this.reply(&id, output.to_result());
            this.running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&task_key);
        });
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, handle.abort_handle());
    }

    fn handle_message(self: &Arc<Self>, msg: Value) {
        let method = msg["method"].as_str().map(str::to_string);
        let id = msg.get("id").cloned();
        match (method, id) {
            (Some(method), Some(id)) => match method.as_str() {
                "initialize" => self.handle_initialize(&id, &msg["params"]),
                "ping" => self.reply(&id, json!({})),
                "tools/list" => self.reply(&id, json!({"tools": tools::definitions()})),
                "tools/call" => self.handle_call(id, msg["params"].clone()),
                other => self.reply_error(&id, -32601, &format!("Method not found: {other}")),
            },
            (Some(method), None) => match method.as_str() {
                "notifications/initialized" | "notifications/roots/list_changed" => {
                    self.request_roots();
                }
                "notifications/cancelled" => {
                    let key = msg["params"]["requestId"].to_string();
                    if let Some(h) = self
                        .running
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&key)
                    {
                        h.abort();
                    }
                }
                _ => {}
            },
            (None, Some(_)) => self.handle_response(&msg),
            (None, None) => {}
        }
    }
}

/// Runs the server on stdin/stdout until stdin closes, then cancels running exports.
pub async fn run(env: Arc<Env>) {
    let (out_tx, mut out_rx) = unbounded_channel::<Value>();
    std::thread::spawn(move || {
        let stdout = std::io::stdout();
        while let Some(v) = out_rx.blocking_recv() {
            let mut lock = stdout.lock();
            if writeln!(lock, "{v}").and_then(|()| lock.flush()).is_err() {
                break;
            }
        }
    });
    let (in_tx, mut in_rx) = unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(l) => {
                    if in_tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let server = Arc::new(Server {
        env: env.clone(),
        out: out_tx,
        client_roots: AtomicBool::new(false),
        roots_pending: AtomicBool::new(false),
        running: Mutex::new(HashMap::new()),
    });
    while let Some(line) = in_rx.recv().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(msg) if msg.is_object() => server.handle_message(msg),
            Ok(_) => server.reply_error(&Value::Null, -32600, "Batches are not supported"),
            Err(e) => server.reply_error(&Value::Null, -32700, &format!("Parse error: {e}")),
        }
    }
    tracing::info!("stdin closed; shutting down");
    crate::export::shutdown(&env).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}
