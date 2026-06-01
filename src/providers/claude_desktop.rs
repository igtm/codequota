use super::claude_common;
use super::error::ProviderError;
use super::model::{ProviderKind, UsageRecord};

pub fn fetch(client: &reqwest::blocking::Client) -> Result<UsageRecord, ProviderError> {
    let credentials = claude_common::load_claude_desktop_credentials()?;
    claude_common::fetch_usage(client, ProviderKind::ClaudeDesktop, credentials)
}
