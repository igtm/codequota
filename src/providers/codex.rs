use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use reqwest::StatusCode;
use serde_json::{Map, Value};

use super::claude_common::{parse_timestamp, take_f64};
use super::error::ProviderError;
use super::model::{ProviderKind, UsageRecord, UsageWindow};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_URL_FALLBACK: &str = "https://chatgpt.com/api/codex/usage";

#[derive(Debug, Eq, PartialEq)]
struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

pub fn fetch(client: &reqwest::blocking::Client) -> Result<UsageRecord, ProviderError> {
    let auth_path = env::var_os("CODEQUOTA_CODEX_AUTH_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex/auth.json"));
    let auth = read_auth_file(&auth_path)?;

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
        match fetch_usage_from_endpoint(client, &auth, &auth_path, &endpoint) {
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
    auth_path: &Path,
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
        StatusCode::OK => parse_usage_payload(&body, auth_path),
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

fn read_auth_file(path: &PathBuf) -> Result<CodexAuth, ProviderError> {
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

    parse_auth_value(&value)
}

fn parse_auth_value(value: &Value) -> Result<CodexAuth, ProviderError> {
    let root = value.as_object().ok_or_else(|| {
        ProviderError::parse(ProviderKind::Codex, "auth payload must be a JSON object")
    })?;
    let tokens = root
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ProviderError::parse(ProviderKind::Codex, "auth payload missing tokens object")
        })?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::parse(
                ProviderKind::Codex,
                "auth payload missing tokens.access_token",
            )
        })?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    Ok(CodexAuth {
        access_token,
        account_id,
    })
}

fn parse_usage_payload(body: &str, auth_path: &Path) -> Result<UsageRecord, ProviderError> {
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
        Some(auth_path.display().to_string()),
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
            parse_timestamp(ProviderKind::Codex, value, &format!("{field}.resetsAt"))
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
                account_id: Some("account-id".to_string())
            }
        );
    }

    #[test]
    fn parses_usage_payload_with_epoch_resets() {
        let auth_path = PathBuf::from("/tmp/auth.json");
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
            &auth_path,
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
        let auth_path = PathBuf::from("/tmp/auth.json");
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
            &auth_path,
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
    fn rejects_missing_windows() {
        let auth_path = PathBuf::from("/tmp/auth.json");
        let error = parse_usage_payload(r#"{"rateLimits":{"planType":"plus"}}"#, &auth_path)
            .expect_err("payload without windows must fail");

        assert!(error.message.contains("primary or secondary windows"));
    }
}
