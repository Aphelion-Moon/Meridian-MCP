use crate::{MeridianServer, ServerConfig};
use anyhow::Result;
use rmcp::{transport::stdio, ServiceExt};
use serde::Serialize;
use serde_json::Value;

pub use crate::result::{ToolContent, ToolResult};

#[derive(Debug, Serialize, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
}

pub async fn run_server(config: ServerConfig) -> Result<()> {
    #[cfg(windows)]
    crate::process::initialize_runtime_owner()?;
    run_transport(MeridianServer::new(config)?, stdio()).await
}

pub(crate) async fn run_transport<R, W>(server: MeridianServer, transport: (R, W)) -> Result<()>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let outcome = match server.clone().serve(transport).await {
        Ok(service) => service
            .waiting()
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from),
        Err(error) => Err(error.into()),
    };
    let shutdown = server.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}
