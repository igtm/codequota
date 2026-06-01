use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Codex => "codex",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatus {
    Ok,
    Error,
    Unsupported,
}

impl ProviderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageWindow {
    pub utilization: f64,
    pub resets_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageRecord {
    pub provider: ProviderKind,
    pub status: ProviderStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<UsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<UsageWindow>,
    pub generated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UsageRecord {
    pub fn success(
        provider: ProviderKind,
        auth_source: Option<String>,
        plan: Option<String>,
        five_hour: Option<UsageWindow>,
        seven_day: Option<UsageWindow>,
    ) -> Self {
        Self {
            provider,
            status: ProviderStatus::Ok,
            auth_source,
            plan,
            five_hour,
            seven_day,
            generated_at: Utc::now(),
            error: None,
        }
    }

    pub fn is_success(&self) -> bool {
        self.status == ProviderStatus::Ok
    }
}
