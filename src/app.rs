use tracing::info;

use crate::{
    cli::Cli,
    error::Result,
    network::{HttpClient, HttpConfig},
    storage::Storage,
    tui::TerminalSession,
};

pub async fn run(cli: Cli) -> Result<()> {
    let storage = Storage::open_default(cli.private)?;
    let settings = storage.settings()?;
    let input = cli.url.as_deref().unwrap_or(&settings.default_homepage);
    info!(%input, "opening website");
    let mut terminal = TerminalSession::new()?;
    let mut http_config = HttpConfig::default();
    http_config.request_timeout = std::time::Duration::from_secs(settings.request_timeout_seconds);
    http_config.user_agent = settings.user_agent;
    terminal
        .run_url(input.to_owned(), HttpClient::new(http_config)?, storage)
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
