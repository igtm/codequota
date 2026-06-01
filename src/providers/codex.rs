use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

use reqwest::StatusCode;
use serde_json::{Map, Value};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};

use super::claude_common::{parse_optional_timestamp, take_f64};
use super::error::ProviderError;
use super::model::{ProviderKind, UsageRecord, UsageWindow};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_URL_FALLBACK: &str = "https://chatgpt.com/api/codex/usage";
const CODEX_AUTH_FILE_ENV: &str = "CODEQUOTA_CODEX_AUTH_FILE";
const CODEX_ACCESS_TOKEN_ENV: &str = "CODEX_ACCESS_TOKEN";
const CODEX_ACCOUNT_ID_ENV: &str = "CODEX_ACCOUNT_ID";
const CHATGPT_ACCOUNT_ID_ENV: &str = "CHATGPT_ACCOUNT_ID";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
#[cfg(target_os = "macos")]
const CODEX_KEYCHAIN_SERVICE: &str = "Codex Auth";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexCredentialsStoreMode {
    File,
    Keyring,
    Auto,
}

#[derive(Debug, Eq, PartialEq)]
struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
    auth_source: String,
}

pub fn fetch(client: &reqwest::blocking::Client) -> Result<UsageRecord, ProviderError> {
    let auth = load_auth()?;

    let usage_override = env::var("CODEQUOTA_CODEX_USAGE_URL").ok();
    let endpoints = if let Some(endpoint) = usage_override {
        vec![endpoint]
    } else {
        vec![
            CODEX_USAGE_URL.to_string(),
            CODEX_USAGE_URL_FALLBACK.to_string(),
        ]
    };

    let mut last_error = None;

    for endpoint in endpoints {
        match fetch_usage_from_endpoint(client, &auth, &endpoint) {
            Ok(record) => return Ok(record),
            Err(error) => {
                if error.message.contains("unexpected HTTP 404")
                    && env::var("CODEQUOTA_CODEX_USAGE_URL").is_err()
                {
                    last_error = Some(error);
                    continue;
                }
                return Err(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ProviderError::http(
            ProviderKind::Codex,
            "no usable Codex usage endpoint succeeded",
        )
    }))
}

fn fetch_usage_from_endpoint(
    client: &reqwest::blocking::Client,
    auth: &CodexAuth,
    endpoint: &str,
) -> Result<UsageRecord, ProviderError> {
    let mut request = client
        .get(endpoint)
        .header("Authorization", format!("Bearer {}", auth.access_token))
        .header("Content-Type", "application/json");

    if let Some(account_id) = &auth.account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request.send().map_err(|error| {
        ProviderError::http(ProviderKind::Codex, format!("request failed: {error}"))
    })?;
    let status = response.status();
    let body = response.text().map_err(|error| {
        ProviderError::http(
            ProviderKind::Codex,
            format!("failed to read response body: {error}"),
        )
    })?;

    match status {
        StatusCode::OK => parse_usage_payload(&body, &auth.auth_source),
        StatusCode::UNAUTHORIZED => Err(ProviderError::auth(
            ProviderKind::Codex,
            format!(
                "401 unauthorized: {}",
                extract_error_message(&body, "unauthorized")
            ),
        )),
        StatusCode::FORBIDDEN => Err(ProviderError::auth(
            ProviderKind::Codex,
            format!(
                "403 forbidden: {}",
                extract_error_message(&body, "forbidden")
            ),
        )),
        StatusCode::TOO_MANY_REQUESTS => Err(ProviderError::http(
            ProviderKind::Codex,
            format!(
                "429 rate limited: {}",
                extract_error_message(&body, "too many requests")
            ),
        )),
        _ => Err(ProviderError::http(
            ProviderKind::Codex,
            format!(
                "unexpected HTTP {}: {}",
                status.as_u16(),
                extract_error_message(&body, "request failed")
            ),
        )),
    }
}

fn load_auth() -> Result<CodexAuth, ProviderError> {
    let codex_home = codex_home();
    let store_mode = codex_config_store_mode(&codex_home);

    if let Ok(access_token) = env::var(CODEX_ACCESS_TOKEN_ENV)
        && !access_token.trim().is_empty()
    {
        return Ok(CodexAuth {
            access_token,
            account_id: env_account_id(),
            auth_source: format!("env:{CODEX_ACCESS_TOKEN_ENV}"),
        });
    }

    if let Some(path) = env::var_os(CODEX_AUTH_FILE_ENV).map(PathBuf::from) {
        return read_auth_file(&path);
    }

    let default_path = codex_home.join("auth.json");

    #[cfg(target_os = "macos")]
    if matches!(
        store_mode,
        Some(CodexCredentialsStoreMode::Keyring) | Some(CodexCredentialsStoreMode::Auto)
    ) {
        match load_auth_from_keychain(&codex_home) {
            Ok(auth) => return Ok(auth),
            Err(_error)
                if matches!(store_mode, Some(CodexCredentialsStoreMode::Auto))
                    && default_path.is_file() =>
            {
                return read_auth_file(&default_path);
            }
            Err(error) if matches!(store_mode, Some(CodexCredentialsStoreMode::Keyring)) => {
                return Err(error);
            }
            Err(_) => {}
        }
    }

    if default_path.is_file() {
        return read_auth_file(&default_path);
    }

    Err(ProviderError::io(
        ProviderKind::Codex,
        format!(
            "failed to read auth file {}: No such file or directory (os error 2)",
            default_path.display()
        ),
    ))
}

fn read_auth_file(path: &Path) -> Result<CodexAuth, ProviderError> {
    let raw = fs::read_to_string(path).map_err(|error| {
        ProviderError::io(
            ProviderKind::Codex,
            format!("failed to read auth file {}: {error}", path.display()),
        )
    })?;

    let value = serde_json::from_str::<Value>(&raw).map_err(|error| {
        ProviderError::parse(
            ProviderKind::Codex,
            format!("invalid auth JSON in {}: {error}", path.display()),
        )
    })?;

    let mut auth = parse_auth_value(&value)?;
    auth.auth_source = path.display().to_string();
    Ok(auth)
}

#[cfg(target_os = "macos")]
fn load_auth_from_keychain(codex_home: &Path) -> Result<CodexAuth, ProviderError> {
    let account = compute_store_key(codex_home);
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            CODEX_KEYCHAIN_SERVICE,
            "-a",
            &account,
            "-w",
        ])
        .output()
        .map_err(|error| {
            ProviderError::io(
                ProviderKind::Codex,
                format!(
                    "failed to execute macOS security command for service {CODEX_KEYCHAIN_SERVICE} account {account}: {error}"
                ),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("security exited with status {}", output.status)
        } else {
            stderr
        };
        return Err(ProviderError::auth(
            ProviderKind::Codex,
            format!(
                "failed to read macOS keychain service {CODEX_KEYCHAIN_SERVICE} account {account}: {message}"
            ),
        ));
    }

    let value = serde_json::from_slice::<Value>(&output.stdout).map_err(|error| {
        ProviderError::parse(
            ProviderKind::Codex,
            format!(
                "invalid auth JSON in macOS keychain service {CODEX_KEYCHAIN_SERVICE}: {error}"
            ),
        )
    })?;

    let mut auth = parse_auth_value(&value)?;
    auth.auth_source = format!("macos-keychain:{CODEX_KEYCHAIN_SERVICE}");
    Ok(auth)
}

#[cfg(target_os = "macos")]
fn compute_store_key(codex_home: &Path) -> String {
    let canonical = codex_home
        .canonicalize()
        .unwrap_or_else(|_| codex_home.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    let truncated = hex.get(..16).unwrap_or(&hex);
    format!("cli|{truncated}")
}

fn parse_auth_value(value: &Value) -> Result<CodexAuth, ProviderError> {
    let root = value.as_object().ok_or_else(|| {
        ProviderError::parse(ProviderKind::Codex, "auth payload must be a JSON object")
    })?;
    let token_candidates = [
        root.get("tokens").and_then(Value::as_object),
        root.get("agent_identity").and_then(Value::as_object),
        Some(root),
    ];
    let access_token =
        first_string_from_objects(&token_candidates, &["access_token", "accessToken"]).ok_or_else(
            || {
                ProviderError::parse(
                    ProviderKind::Codex,
                    "auth payload missing tokens.access_token",
                )
            },
        )?;
    let account_id = first_string_from_objects(
        &token_candidates,
        &[
            "account_id",
            "accountId",
            "chatgpt_account_id",
            "chatgptAccountId",
        ],
    )
    .or_else(env_account_id);

    Ok(CodexAuth {
        access_token,
        account_id,
        auth_source: "unknown".to_string(),
    })
}

fn parse_usage_payload(body: &str, auth_source: &str) -> Result<UsageRecord, ProviderError> {
    let payload = serde_json::from_str::<Value>(body).map_err(|error| {
        ProviderError::parse(
            ProviderKind::Codex,
            format!("invalid JSON response: {error}"),
        )
    })?;

    let root = payload.as_object().ok_or_else(|| {
        ProviderError::parse(
            ProviderKind::Codex,
            "usage payload must be a JSON object with rate limits",
        )
    })?;

    let (plan, five_hour, seven_day) =
        if let Some(rate_limit) = root.get("rate_limit").and_then(Value::as_object) {
            (
                first_string(root, &["plan_type", "planType"])
                    .or_else(|| first_string(rate_limit, &["plan_type", "planType", "limitName"])),
                parse_window(rate_limit.get("primary_window"), "primary_window")?,
                parse_window(rate_limit.get("secondary_window"), "secondary_window")?,
            )
        } else {
            let rate_limits = find_rate_limits(&payload)?;
            (
                first_string(root, &["plan_type", "planType"])
                    .or_else(|| first_string(rate_limits, &["planType", "plan_type", "limitName"])),
                parse_window(rate_limits.get("primary"), "primary")?,
                parse_window(rate_limits.get("secondary"), "secondary")?,
            )
        };

    if five_hour.is_none() && seven_day.is_none() {
        return Err(ProviderError::parse(
            ProviderKind::Codex,
            "response did not contain primary or secondary windows",
        ));
    }

    Ok(UsageRecord::success(
        ProviderKind::Codex,
        Some(auth_source.to_string()),
        plan,
        five_hour,
        seven_day,
    ))
}

fn find_rate_limits(payload: &Value) -> Result<&Map<String, Value>, ProviderError> {
    if let Some(object) = payload.get("rateLimits").and_then(Value::as_object) {
        return Ok(object);
    }
    if let Some(object) = payload.get("rate_limits").and_then(Value::as_object) {
        return Ok(object);
    }
    if let Some(object) = payload
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .and_then(|map| map.values().find_map(Value::as_object))
    {
        return Ok(object);
    }
    payload.as_object().ok_or_else(|| {
        ProviderError::parse(
            ProviderKind::Codex,
            "usage payload must be a JSON object with rate limits",
        )
    })
}

fn parse_window(value: Option<&Value>, field: &str) -> Result<Option<UsageWindow>, ProviderError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = value.as_object().ok_or_else(|| {
        ProviderError::parse(
            ProviderKind::Codex,
            format!("{field} must be a JSON object"),
        )
    })?;

    let utilization = object
        .get("usedPercent")
        .or_else(|| object.get("used_percent"))
        .ok_or_else(|| {
            ProviderError::parse(ProviderKind::Codex, format!("{field}.usedPercent missing"))
        })
        .and_then(|value| take_f64(ProviderKind::Codex, value, &format!("{field}.usedPercent")))?;

    let resets_at = object
        .get("resetsAt")
        .or_else(|| object.get("resets_at"))
        .or_else(|| object.get("reset_at"))
        .ok_or_else(|| {
            ProviderError::parse(ProviderKind::Codex, format!("{field}.resetsAt missing"))
        })
        .and_then(|value| {
            parse_optional_timestamp(ProviderKind::Codex, value, &format!("{field}.resetsAt"))
        })?;

    Ok(Some(UsageWindow {
        utilization,
        resets_at,
    }))
}

fn first_string(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(Value::as_str)
        .map(ToOwned::to_owned)
}

fn first_string_from_objects(
    objects: &[Option<&Map<String, Value>>],
    keys: &[&str],
) -> Option<String> {
    objects
        .iter()
        .flatten()
        .find_map(|object| first_string(object, keys))
}

fn env_account_id() -> Option<String> {
    [CODEX_ACCOUNT_ID_ENV, CHATGPT_ACCOUNT_ID_ENV]
        .into_iter()
        .find_map(|key| env::var(key).ok())
        .filter(|value| !value.trim().is_empty())
}

fn codex_config_store_mode(codex_home: &Path) -> Option<CodexCredentialsStoreMode> {
    let config_path = codex_home.join("config.toml");
    let Ok(raw) = fs::read_to_string(config_path) else {
        return None;
    };

    codex_config_store_mode_from_raw(&raw)
}

fn codex_config_store_mode_from_raw(raw: &str) -> Option<CodexCredentialsStoreMode> {
    raw.lines().find_map(|line| {
        let trimmed = line.trim();
        if !trimmed.starts_with("cli_auth_credentials_store") {
            return None;
        }

        let (_, value) = trimmed.split_once('=')?;
        match value.trim().trim_matches('"') {
            "file" => Some(CodexCredentialsStoreMode::File),
            "keyring" => Some(CodexCredentialsStoreMode::Keyring),
            "auto" => Some(CodexCredentialsStoreMode::Auto),
            _ => None,
        }
    })
}

fn extract_error_message(body: &str, fallback: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|payload| {
            payload
                .get("error")
                .and_then(Value::as_str)
                .or_else(|| payload.get("message").and_then(Value::as_str))
                .or_else(|| payload.get("detail").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                fallback.to_string()
            } else {
                trimmed.to_string()
            }
        })
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must be set on supported platforms")
}

fn codex_home() -> PathBuf {
    env::var_os(CODEX_HOME_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_fixture() {
        let value = serde_json::from_str::<Value>(
            r#"{
              "auth_mode": "chatgpt",
              "tokens": {
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "account_id": "account-id"
              }
            }"#,
        )
        .expect("fixture should parse");

        let auth = parse_auth_value(&value).expect("auth fixture should parse");

        assert_eq!(
            auth,
            CodexAuth {
                access_token: "access-token".to_string(),
                account_id: Some("account-id".to_string()),
                auth_source: "unknown".to_string(),
            }
        );
    }

    #[test]
    fn parses_auth_with_top_level_fields() {
        let value = serde_json::from_str::<Value>(
            r#"{
              "access_token": "access-token",
              "chatgpt_account_id": "account-id"
            }"#,
        )
        .expect("fixture should parse");

        let auth = parse_auth_value(&value).expect("top-level auth fixture should parse");

        assert_eq!(
            auth,
            CodexAuth {
                access_token: "access-token".to_string(),
                account_id: Some("account-id".to_string()),
                auth_source: "unknown".to_string(),
            }
        );
    }

    #[test]
    fn parses_usage_payload_with_epoch_resets() {
        let record = parse_usage_payload(
            r#"{
              "rateLimits": {
                "planType": "plus",
                "primary": {
                  "usedPercent": 12,
                  "windowDurationMins": 300,
                  "resetsAt": 1777534802
                },
                "secondary": {
                  "usedPercent": 67,
                  "windowDurationMins": 10080,
                  "resetsAt": 1777969707
                }
              }
            }"#,
            "/tmp/auth.json",
        )
        .expect("usage payload should parse");

        assert_eq!(record.plan.as_deref(), Some("plus"));
        assert_eq!(
            record.five_hour.as_ref().map(|window| window.utilization),
            Some(12.0)
        );
        assert_eq!(
            record.seven_day.as_ref().map(|window| window.utilization),
            Some(67.0)
        );
    }

    #[test]
    fn parses_live_wham_usage_shape() {
        let record = parse_usage_payload(
            r#"{
              "plan_type": "plus",
              "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                  "used_percent": 4,
                  "limit_window_seconds": 18000,
                  "reset_after_seconds": 17833,
                  "reset_at": 1780291327
                },
                "secondary_window": {
                  "used_percent": 1,
                  "limit_window_seconds": 604800,
                  "reset_after_seconds": 604633,
                  "reset_at": 1780878127
                }
              }
            }"#,
            "/tmp/auth.json",
        )
        .expect("live usage payload shape should parse");

        assert_eq!(record.plan.as_deref(), Some("plus"));
        assert_eq!(
            record.five_hour.as_ref().map(|window| window.utilization),
            Some(4.0)
        );
        assert_eq!(
            record.seven_day.as_ref().map(|window| window.utilization),
            Some(1.0)
        );
    }

    #[test]
    fn parses_usage_payload_with_null_reset() {
        let record = parse_usage_payload(
            r#"{
              "rateLimits": {
                "planType": "plus",
                "primary": {
                  "usedPercent": 0,
                  "resetsAt": null
                }
              }
            }"#,
            "/tmp/auth.json",
        )
        .expect("usage payload with null reset should parse");

        assert!(
            record
                .five_hour
                .as_ref()
                .is_some_and(|window| window.resets_at.is_none())
        );
    }

    #[test]
    fn detects_keyring_config() {
        assert_eq!(
            codex_config_store_mode_from_raw(r#"cli_auth_credentials_store = "keyring""#),
            Some(CodexCredentialsStoreMode::Keyring)
        );
        assert_eq!(
            codex_config_store_mode_from_raw(r#"cli_auth_credentials_store = "auto""#),
            Some(CodexCredentialsStoreMode::Auto)
        );
        assert_eq!(
            codex_config_store_mode_from_raw(r#"cli_auth_credentials_store = "file""#),
            Some(CodexCredentialsStoreMode::File)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn computes_macos_store_key_shape() {
        let key = compute_store_key(Path::new("/tmp/example-codex-home"));
        assert!(key.starts_with("cli|"));
        assert_eq!(key.len(), 20);
    }

    #[test]
    fn rejects_missing_windows() {
        let error = parse_usage_payload(r#"{"rateLimits":{"planType":"plus"}}"#, "/tmp/auth.json")
            .expect_err("payload without windows must fail");

        assert!(error.message.contains("primary or secondary windows"));
    }
}
