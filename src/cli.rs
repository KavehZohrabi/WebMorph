use clap::{Parser, ValueEnum};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Verbosity {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EngineChoice {
    Lightweight,
    Chromium,
}

impl Verbosity {
    fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Transform a website into an interactive terminal view"
)]
pub struct Cli {
    /// Website URL to open
    pub url: Option<String>,

    /// Do not read or write persistent browsing history for this session
    #[arg(long)]
    pub private: bool,

    /// Select the rendering engine (Chromium requires `--features browser-engine`)
    #[arg(long, value_enum, default_value = "lightweight")]
    pub engine: EngineChoice,

    /// Set log verbosity (overridden by RUST_LOG when present)
    #[arg(short, long, value_enum, default_value = "warn")]
    pub verbosity: Verbosity,
}

impl Cli {
    pub fn init_logging(&self) {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(self.verbosity.as_filter()));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, EngineChoice, Verbosity};

    #[test]
    fn parses_url_and_verbosity() {
        let cli = Cli::try_parse_from(["webmorph", "https://example.com", "--verbosity", "debug"])
            .expect("valid arguments");
        assert_eq!(cli.url.as_deref(), Some("https://example.com"));
        assert!(matches!(cli.verbosity, Verbosity::Debug));
    }

    #[test]
    fn chromium_engine_requires_explicit_selection() {
        let cli = Cli::try_parse_from(["webmorph", "--engine", "chromium", "https://example.com"])
            .expect("valid Chromium engine option");
        assert!(matches!(cli.engine, EngineChoice::Chromium));
    }
}
