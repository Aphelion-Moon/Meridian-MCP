use anyhow::Result;
use meridian_mcp::ServerConfig;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn default_rust_log_filter() -> String {
    std::env::var("RUST_LOG").unwrap_or_else(|_| "meridian_mcp=info".into())
}

fn main() -> Result<()> {
    if meridian_mcp::authoring_export::dispatch_cli(
        &std::env::args_os().skip(1).collect::<Vec<_>>(),
    )? {
        return Ok(());
    }
    #[cfg(unix)]
    {
        if meridian_mcp::process::dispatch_runtime_guardian() {
            return Ok(());
        }
        meridian_mcp::process::initialize_runtime_owner_with_executable(&std::env::current_exe()?)?;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(default_rust_log_filter()))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let config = ServerConfig::from_env()?;
    info!(mode = ?config.mode(), root_count = config.workspace_roots().len(), "starting Meridian-MCP");
    meridian_mcp::run(config).await
}

#[cfg(test)]
mod tests {
    use super::default_rust_log_filter;

    #[test]
    fn default_rust_log_filter_uses_meridian_target() {
        unsafe { std::env::remove_var("RUST_LOG") };
        assert_eq!(default_rust_log_filter(), "meridian_mcp=info");
    }
}
