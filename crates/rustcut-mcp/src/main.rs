//! `rustcut-mcp`: stdio MCP server. stdout is reserved for the protocol.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    let env = rustcut_mcp::env::Env::from_process();
    let file = std::fs::create_dir_all(&env.paths.log)
        .ok()
        .map(|()| tracing_appender::rolling::never(&env.paths.log, "mcp.log"));
    let filter = tracing_subscriber::EnvFilter::try_from_env("RUSTCUT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(false);
    let registry = tracing_subscriber::registry().with(filter).with(stderr);
    match file {
        Some(f) => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(f)
                    .with_ansi(false),
            )
            .init(),
        None => registry.init(),
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), data = %env.paths.data.display(), "rustcut-mcp starting");
    let guard = rustcut_mcp::procguard::install();
    tracing::info!(
        child_kill_guard = guard,
        "child processes end with the server"
    );
    if !env.policy.is_restricted() {
        tracing::warn!(
            "no RUSTCUT_MCP_ROOTS set: all paths are allowed (a client that sends roots can still restrict this)"
        );
    }
    rustcut_mcp::rpc::run(env).await;
}
