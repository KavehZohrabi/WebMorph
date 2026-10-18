use clap::Parser;
use webmorph::{app, cli::Cli, error::Result};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("webmorph: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    cli.init_logging();
    app::run(cli).await
}
