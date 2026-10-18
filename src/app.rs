use clap::CommandFactory;
use tracing::info;

use crate::{cli::Cli, error::Result, network::HttpClient, tui::TerminalSession};

pub async fn run(cli: Cli) -> Result<()> {
    let Some(input) = cli.url.as_deref() else {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    info!(%input, "opening website");
    let mut terminal = TerminalSession::new()?;
    terminal
        .run_url(input.to_owned(), HttpClient::default())
        .await
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn no_url_is_a_valid_startup_state() {
        let cli = Cli::try_parse_from(["webmorph"]).expect("no URL is allowed");
        assert!(cli.url.is_none());
    }
}
