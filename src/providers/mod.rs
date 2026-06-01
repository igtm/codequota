pub mod claude_code;
pub mod claude_common;
pub mod claude_desktop;
pub mod codex;
pub mod error;
pub mod model;
pub mod output;

use std::time::Duration;

use model::{ProviderKind, UsageRecord};

use crate::providers::error::ProviderError;

pub fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!("codequota/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("building HTTP client should not fail")
}

pub fn fetch(
    provider: ProviderKind,
    client: &reqwest::blocking::Client,
) -> Result<UsageRecord, ProviderError> {
    match provider {
        ProviderKind::ClaudeCode => claude_code::fetch(client),
        ProviderKind::ClaudeDesktop => claude_desktop::fetch(client),
        ProviderKind::Codex => codex::fetch(client),
    }
}

pub fn all_providers() -> [ProviderKind; 3] {
    [
        ProviderKind::ClaudeCode,
        ProviderKind::ClaudeDesktop,
        ProviderKind::Codex,
    ]
}
