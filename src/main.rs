use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    knot::cli::run().await
}
