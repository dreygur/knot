#![allow(dead_code)]

mod engine;
mod jitv;
mod logging;
mod memory;
mod tools;

use anyhow::Result;
use engine::StorageEngine;
use rmcp::{transport::stdio, ServiceExt};
use tools::KnotServer;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=info".into());
    logging::init(&filter);

    let data_dir = std::env::var("KNOT_DATA_DIR")
        .unwrap_or_else(|_| resolve_data_dir());

    tracing::info!("v{} data_dir={data_dir}", env!("CARGO_PKG_VERSION"));

    let engine = StorageEngine::new(&data_dir).await?;
    let session_id = Uuid::new_v4().to_string();
    tracing::info!("session={session_id}");

    let server = KnotServer::new(engine, session_id);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}

fn resolve_data_dir() -> String {
    std::env::var("HOME")
        .map(|h| format!("{h}/.knot"))
        .unwrap_or_else(|_| ".knot".into())
}
