#[tokio::main]
async fn main() {
    if let Err(e) = knot::cli::run().await {
        eprintln!("✗ {e}");
        for cause in e.chain().skip(1) {
            eprintln!("  caused by: {cause}");
        }
        std::process::exit(1);
    }
}
