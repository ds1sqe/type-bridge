mod config;
mod error;
mod interceptor;
mod server;
mod transport;

use std::net::SocketAddr;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "type-bridge-server", version, about = "TypeDB query proxy server")]
struct Cli {
    /// Path to the server configuration file
    #[arg(short, long, default_value = "server.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Load config
    let config = config::ServerConfig::from_file(&cli.config)?;

    // Initialize logging
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.logging.level));

    match config.logging.format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .init();
        }
    }

    tracing::info!(
        host = config.server.host.as_str(),
        port = config.server.port,
        database = config.typedb.database.as_str(),
        "Starting type-bridge-server"
    );

    // Build application state
    let state = server::build_app_state(&config)?;

    // Build router
    let router = transport::http::create_router(state);

    // Start server
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .map_err(|e| format!("Invalid listen address: {}", e))?;

    tracing::info!(%addr, "Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
