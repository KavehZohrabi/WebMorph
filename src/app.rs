use clap::CommandFactory;
use tracing::{error, info};

use crate::{
    cli::Cli, error::Result, html, network::HttpClient, plugins::PluginRegistry, transform,
    tui::TerminalSession,
};

pub async fn run(cli: Cli) -> Result<()> {
    let Some(input) = cli.url.as_deref() else {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    let url = HttpClient::parse_url(input)?;
    info!(%url, "opening website");
    let client = HttpClient::default();
    let source = tokio::select! {
        result = client.get_text(&url) => result.map_err(|error| {
            error!(%url, %error, "could not fetch website");
            error
        })?,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            info!("shutdown requested");
            return Ok(());
        }
    };
    let mut document = html::parse(url.as_str(), &source);
    PluginRegistry::default().process(&mut document);
    let page = transform::transform(&document);
    let mut terminal = TerminalSession::new()?;
    terminal.run(&page).await
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
