use thiserror::Error;

use super::model::{ProviderKind, ProviderStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Auth,
    Http,
    Io,
    Parse,
    Unsupported,
}

impl ErrorKind {
    pub fn status(self) -> ProviderStatus {
        match self {
            Self::Unsupported => ProviderStatus::Unsupported,
            Self::Auth | Self::Http | Self::Io | Self::Parse => ProviderStatus::Error,
        }
    }
}

#[derive(Debug, Error)]
#[error("{provider}: {message}")]
pub struct ProviderError {
    pub provider: ProviderKind,
    pub kind: ErrorKind,
    pub message: String,
}

impl ProviderError {
    pub fn auth(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ErrorKind::Auth,
            message: message.into(),
        }
    }

    pub fn http(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ErrorKind::Http,
            message: message.into(),
        }
    }

    pub fn io(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ErrorKind::Io,
            message: message.into(),
        }
    }

    pub fn parse(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ErrorKind::Parse,
            message: message.into(),
        }
    }

    pub fn unsupported(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ErrorKind::Unsupported,
            message: message.into(),
        }
    }
}
