use std::ffi::OsString;
use std::io;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::providers::error::ProviderError;
use crate::providers::model::{ProviderKind, ProviderStatus, UsageRecord};
use crate::providers::{self, output};

#[derive(Debug, Parser)]
#[command(
    name = "codequota",
    version,
    about = "Show usage limits for Claude Code, Claude Desktop, and Codex.",
    disable_help_subcommand = true,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[arg(long, help = "Render machine-readable JSON output.")]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Usage {
        #[arg(long, help = "Render machine-readable JSON output.")]
        json: bool,
    },
    ClaudeCode {
        #[arg(long, help = "Render machine-readable JSON output.")]
        json: bool,
    },
    ClaudeDesktop {
        #[arg(long, help = "Render machine-readable JSON output.")]
        json: bool,
    },
    Codex {
        #[arg(long, help = "Render machine-readable JSON output.")]
        json: bool,
    },
    Help {
        #[arg(value_enum)]
        topic: Option<HelpTopic>,
    },
    Version,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HelpTopic {
    Usage,
    ClaudeCode,
    ClaudeDesktop,
    Codex,
    Help,
    Version,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestedTarget {
    All,
    Single(ProviderKind),
}

pub fn run() -> i32 {
    run_from(std::env::args_os())
}

pub fn run_from<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).unwrap_or_else(|error| error.exit());

    match cli.command {
        Some(Commands::Help { topic }) => {
            if let Err(error) = print_help(topic) {
                eprintln!("failed to write help output: {error}");
                return 1;
            }
            0
        }
        Some(Commands::Version) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(Commands::Usage { json }) => execute(RequestedTarget::All, json),
        None => execute(RequestedTarget::All, cli.json),
        Some(Commands::ClaudeCode { json }) => {
            execute(RequestedTarget::Single(ProviderKind::ClaudeCode), json)
        }
        Some(Commands::ClaudeDesktop { json }) => {
            execute(RequestedTarget::Single(ProviderKind::ClaudeDesktop), json)
        }
        Some(Commands::Codex { json }) => {
            execute(RequestedTarget::Single(ProviderKind::Codex), json)
        }
    }
}

fn execute(target: RequestedTarget, json: bool) -> i32 {
    let client = providers::http_client();
    let records = match target {
        RequestedTarget::All => collect_all(&client),
        RequestedTarget::Single(provider) => vec![collect_single(provider, &client)],
    };

    if let Err(error) = emit_output(target, json, &records) {
        eprintln!("failed to render output: {error}");
        return 1;
    }

    exit_code(target, &records)
}

fn collect_all(client: &reqwest::blocking::Client) -> Vec<UsageRecord> {
    providers::all_providers()
        .into_iter()
        .map(|provider| collect_single(provider, client))
        .collect()
}

fn collect_single(provider: ProviderKind, client: &reqwest::blocking::Client) -> UsageRecord {
    match providers::fetch(provider, client) {
        Ok(record) => record,
        Err(error) => UsageRecord::from_error(error),
    }
}

fn emit_output(target: RequestedTarget, json: bool, records: &[UsageRecord]) -> io::Result<()> {
    if json {
        match target {
            RequestedTarget::All => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(records)
                        .expect("serializing usage records should not fail")
                );
            }
            RequestedTarget::Single(_) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        records
                            .first()
                            .expect("single-provider output always has exactly one record")
                    )
                    .expect("serializing usage record should not fail")
                );
            }
        }
    } else {
        println!("{}", output::render_human(records));
    }

    Ok(())
}

fn exit_code(target: RequestedTarget, records: &[UsageRecord]) -> i32 {
    match target {
        RequestedTarget::Single(_) => {
            if records
                .first()
                .is_some_and(|record| record.status == ProviderStatus::Ok)
            {
                0
            } else {
                1
            }
        }
        RequestedTarget::All => {
            if records.iter().any(UsageRecord::is_success) {
                0
            } else {
                1
            }
        }
    }
}

fn print_help(topic: Option<HelpTopic>) -> io::Result<()> {
    let mut command = Cli::command();
    match topic {
        None => command.print_long_help(),
        Some(topic) => {
            if let Some(subcommand) = command.find_subcommand_mut(topic.as_str()) {
                subcommand.print_long_help()
            } else {
                command.print_long_help()
            }
        }
    }?;
    println!();
    Ok(())
}

impl HelpTopic {
    fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Codex => "codex",
            Self::Help => "help",
            Self::Version => "version",
        }
    }
}

impl UsageRecord {
    fn from_error(error: ProviderError) -> Self {
        let status = error.kind.status();
        let provider = error.provider;
        Self {
            provider,
            status,
            auth_source: None,
            plan: None,
            five_hour: None,
            seven_day: None,
            generated_at: chrono::Utc::now(),
            error: Some(error.message),
        }
    }
}
