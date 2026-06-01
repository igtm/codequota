use std::env;
use std::fs;
#[cfg(not(target_os = "macos"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

use chrono::{DateTime, TimeZone, Utc};
use reqwest::StatusCode;
use serde_json::{Map, Value, json};

use super::error::ProviderError;
use super::model::{ProviderKind, UsageRecord, UsageWindow};

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_USAGE_BETA: &str = "oauth-2025-04-20";
const CLAUDE_REFRESH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_REFRESH_URL_PRIMARY: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_REFRESH_URL_FALLBACK: &str = "https://console.anthropic.com/v1/oauth/token";
#[cfg(target_os = "macos")]
const CLAUDE_CODE_SERVICE: &str = "Claude Code-credentials";

#[derive(Clone, Debug)]
pub struct ClaudeCredentials {
    access_token: String,
    refresh_token: Option<String>,
    auth_source: String,
    plan_hint: Option<String>,
    storage: CredentialStorage,
}

#[derive(Clone, Debug)]
enum CredentialStorage {
    Env,
    File {
        path: PathBuf,
        root: Value,
    },
    #[cfg(target_os = "macos")]
    Keychain {
        service: String,
        root: Value,
    },
}

#[derive(Debug)]
enum UsageRequestError {
    Unauthorized(String),
    Terminal(ProviderError),
}

#[derive(Clone, Copy)]
enum RefreshContentType {
    Json,
    Form,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

pub fn load_claude_code_credentials() -> Result<ClaudeCredentials, ProviderError> {
    let provider = ProviderKind::ClaudeCode;
    if let Ok(token) = env::var("CLAUDE_CODE_OAUTH_TOKEN")
        && !token.trim().is_empty()
    {
        return Ok(ClaudeCredentials {
            access_token: token,
            refresh_token: None,
            auth_source: "env:CLAUDE_CODE_OAUTH_TOKEN".to_string(),
            plan_hint: None,
            storage: CredentialStorage::Env,
        });
    }

    #[cfg(target_os = "macos")]
    {
        load_keychain_credentials(provider, &[CLAUDE_CODE_SERVICE])
    }

    #[cfg(not(target_os = "macos"))]
    {
        let path = env::var_os("CODEQUOTA_CLAUDE_CREDENTIALS_FILE")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("CLAUDE_CONFIG_DIR")
                    .map(PathBuf::from)
                    .map(|path| path.join(".credentials.json"))
            })
            .unwrap_or_else(|| home_dir().join(".claude/.credentials.json"));
        load_file_credentials(provider, &path)
    }
}

pub fn load_claude_desktop_credentials() -> Result<ClaudeCredentials, ProviderError> {
    let provider = ProviderKind::ClaudeDesktop;

    #[cfg(target_os = "linux")]
    {
        Err(ProviderError::unsupported(provider, "unsupported on Linux"))
    }

    #[cfg(target_os = "macos")]
    {
        let service_override = env::var("CODEQUOTA_CLAUDE_DESKTOP_KEYCHAIN_SERVICE").ok();
        if let Some(service) = service_override {
            return load_keychain_credentials(provider, &[service.as_str()]);
        }

        load_keychain_credentials(
            provider,
            &[
                "Claude Desktop",
                "Claude",
                "Claude-credentials",
                "com.anthropic.Claude",
                CLAUDE_CODE_SERVICE,
            ],
        )
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(ProviderError::unsupported(
            provider,
            format!("unsupported on {}", env::consts::OS),
        ))
    }
}

pub fn fetch_usage(
    client: &reqwest::blocking::Client,
    provider: ProviderKind,
    mut credentials: ClaudeCredentials,
) -> Result<UsageRecord, ProviderError> {
    match fetch_usage_once(client, provider, &credentials) {
        Ok(record) => Ok(record),
        Err(UsageRequestError::Unauthorized(message)) => {
            let refresh_token = credentials.refresh_token.clone().ok_or_else(|| {
                ProviderError::auth(
                    provider,
                    format!("401 unauthorized and no refresh token available: {message}"),
                )
            })?;
            let refreshed = refresh_access_token(client, provider, &refresh_token)?;
            credentials.access_token = refreshed.access_token;
            if let Some(refresh_token) = refreshed.refresh_token {
                credentials.refresh_token = Some(refresh_token);
            }
            credentials.persist(provider)?;
            fetch_usage_once(client, provider, &credentials).map_err(|error| match error {
                UsageRequestError::Unauthorized(message) => ProviderError::auth(
                    provider,
                    format!("401 unauthorized after refresh: {message}"),
                ),
                UsageRequestError::Terminal(error) => error,
            })
        }
        Err(UsageRequestError::Terminal(error)) => Err(error),
    }
}

fn fetch_usage_once(
    client: &reqwest::blocking::Client,
    provider: ProviderKind,
    credentials: &ClaudeCredentials,
) -> Result<UsageRecord, UsageRequestError> {
    let usage_url =
        env::var("CODEQUOTA_ANTHROPIC_USAGE_URL").unwrap_or_else(|_| CLAUDE_USAGE_URL.to_string());

    let response = client
        .get(usage_url)
        .header(
            "Authorization",
            format!("Bearer {}", credentials.access_token),
        )
        .header("anthropic-beta", CLAUDE_USAGE_BETA)
        .header("Content-Type", "application/json")
        .send()
        .map_err(|error| {
            UsageRequestError::Terminal(ProviderError::http(
                provider,
                format!("request failed: {error}"),
            ))
        })?;

    let status = response.status();
    let body = response.text().map_err(|error| {
        UsageRequestError::Terminal(ProviderError::http(
            provider,
            format!("failed to read response body: {error}"),
        ))
    })?;

    match status {
        StatusCode::OK => parse_usage_payload(provider, &body, credentials),
        StatusCode::UNAUTHORIZED => Err(UsageRequestError::Unauthorized(extract_error_message(
            &body,
            "unauthorized",
        ))),
        StatusCode::FORBIDDEN => Err(UsageRequestError::Terminal(ProviderError::auth(
            provider,
            format!(
                "403 forbidden: {}",
                extract_error_message(&body, "forbidden")
            ),
        ))),
        StatusCode::TOO_MANY_REQUESTS => Err(UsageRequestError::Terminal(ProviderError::http(
            provider,
            format!(
                "429 rate limited: {}",
                extract_error_message(&body, "too many requests")
            ),
        ))),
        _ => Err(UsageRequestError::Terminal(ProviderError::http(
            provider,
            format!(
                "unexpected HTTP {}: {}",
                status.as_u16(),
                extract_error_message(&body, "request failed")
            ),
        ))),
    }
}

fn parse_usage_payload(
    provider: ProviderKind,
    body: &str,
    credentials: &ClaudeCredentials,
) -> Result<UsageRecord, UsageRequestError> {
    let payload: Value = serde_json::from_str(body).map_err(|error| {
        UsageRequestError::Terminal(ProviderError::parse(
            provider,
            format!("invalid JSON response: {error}"),
        ))
    })?;

    let five_hour = parse_window(provider, payload.get("five_hour"), "five_hour")
        .map_err(UsageRequestError::Terminal)?;
    let seven_day = parse_window(provider, payload.get("seven_day"), "seven_day")
        .map_err(UsageRequestError::Terminal)?;

    if five_hour.is_none() && seven_day.is_none() {
        return Err(UsageRequestError::Terminal(ProviderError::parse(
            provider,
            "response did not contain five_hour or seven_day windows",
        )));
    }

    Ok(UsageRecord::success(
        provider,
        Some(credentials.auth_source.clone()),
        first_string(
            &[payload.as_object()],
            &[
                "plan",
                "plan_type",
                "planType",
                "subscriptionType",
                "subscription_type",
                "rateLimitTier",
            ],
        )
        .or_else(|| credentials.plan_hint.clone()),
        five_hour,
        seven_day,
    ))
}

fn parse_window(
    provider: ProviderKind,
    value: Option<&Value>,
    field: &str,
) -> Result<Option<UsageWindow>, ProviderError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let object = value
        .as_object()
        .ok_or_else(|| ProviderError::parse(provider, format!("{field} must be a JSON object")))?;

    let utilization = object
        .get("utilization")
        .ok_or_else(|| ProviderError::parse(provider, format!("{field}.utilization missing")))
        .and_then(|value| take_f64(provider, value, &format!("{field}.utilization")))?;

    let resets_at = object
        .get("resets_at")
        .or_else(|| object.get("resetsAt"))
        .ok_or_else(|| ProviderError::parse(provider, format!("{field}.resets_at missing")))
        .and_then(|value| parse_timestamp(provider, value, &format!("{field}.resets_at")))?;

    Ok(Some(UsageWindow {
        utilization,
        resets_at,
    }))
}

fn refresh_access_token(
    client: &reqwest::blocking::Client,
    provider: ProviderKind,
    refresh_token: &str,
) -> Result<RefreshedTokens, ProviderError> {
    let endpoint_override = env::var("CODEQUOTA_ANTHROPIC_TOKEN_URL").ok();
    let endpoints: Vec<String> = if let Some(endpoint) = endpoint_override {
        vec![endpoint]
    } else {
        vec![
            CLAUDE_REFRESH_URL_PRIMARY.to_string(),
            CLAUDE_REFRESH_URL_FALLBACK.to_string(),
        ]
    };

    let mut last_error = None;

    for endpoint in endpoints {
        for content_type in [RefreshContentType::Json, RefreshContentType::Form] {
            match send_refresh_request(client, provider, &endpoint, refresh_token, content_type) {
                Ok(tokens) => return Ok(tokens),
                Err(error) => last_error = Some(error),
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ProviderError::auth(provider, "refresh failed for all configured endpoints")
    }))
}

fn send_refresh_request(
    client: &reqwest::blocking::Client,
    provider: ProviderKind,
    endpoint: &str,
    refresh_token: &str,
    content_type: RefreshContentType,
) -> Result<RefreshedTokens, ProviderError> {
    let response = match content_type {
        RefreshContentType::Json => client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLAUDE_REFRESH_CLIENT_ID,
            }))
            .send(),
        RefreshContentType::Form => client
            .post(endpoint)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", CLAUDE_REFRESH_CLIENT_ID),
            ])
            .send(),
    }
    .map_err(|error| {
        ProviderError::http(
            provider,
            format!("refresh request to {endpoint} failed: {error}"),
        )
    })?;

    let status = response.status();
    let body = response.text().map_err(|error| {
        ProviderError::http(
            provider,
            format!("failed to read refresh response body: {error}"),
        )
    })?;
    if !status.is_success() {
        return Err(ProviderError::auth(
            provider,
            format!(
                "refresh failed with HTTP {}: {}",
                status.as_u16(),
                extract_error_message(&body, "refresh request failed")
            ),
        ));
    }

    let payload: Value = serde_json::from_str(&body).map_err(|error| {
        ProviderError::parse(provider, format!("invalid refresh JSON response: {error}"))
    })?;
    let object = payload
        .as_object()
        .ok_or_else(|| ProviderError::parse(provider, "refresh response must be a JSON object"))?;

    let access_token = first_string(&[Some(object)], &["access_token", "accessToken"])
        .ok_or_else(|| ProviderError::parse(provider, "refresh response missing access_token"))?;
    let refresh_token = first_string(&[Some(object)], &["refresh_token", "refreshToken"]);

    Ok(RefreshedTokens {
        access_token,
        refresh_token,
    })
}

#[cfg(not(target_os = "macos"))]
fn load_file_credentials(
    provider: ProviderKind,
    path: &Path,
) -> Result<ClaudeCredentials, ProviderError> {
    let raw = fs::read_to_string(path).map_err(|error| {
        ProviderError::io(
            provider,
            format!("failed to read credential file {}: {error}", path.display()),
        )
    })?;
    let root = parse_credentials_json(provider, &raw)?;
    build_credentials(
        provider,
        root,
        CredentialStorage::File {
            path: path.to_path_buf(),
            root: parse_credentials_json(provider, &raw)?,
        },
        path.display().to_string(),
    )
}

#[cfg(target_os = "macos")]
fn load_keychain_credentials(
    provider: ProviderKind,
    services: &[&str],
) -> Result<ClaudeCredentials, ProviderError> {
    let mut last_error = None;

    for service in services {
        match read_keychain_password(provider, service) {
            Ok(raw) => {
                let root = parse_credentials_json(provider, &raw)?;
                return build_credentials(
                    provider,
                    root,
                    CredentialStorage::Keychain {
                        service: (*service).to_string(),
                        root: parse_credentials_json(provider, &raw)?,
                    },
                    format!("macos-keychain:{service}"),
                );
            }
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ProviderError::auth(
            provider,
            "no supported macOS keychain credential entry found",
        )
    }))
}

#[cfg(target_os = "macos")]
fn read_keychain_password(provider: ProviderKind, service: &str) -> Result<String, ProviderError> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .map_err(|error| {
            ProviderError::io(
                provider,
                format!("failed to execute security for service {service}: {error}"),
            )
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(ProviderError::auth(
            provider,
            format!(
                "security find-generic-password failed for {service}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

fn build_credentials(
    provider: ProviderKind,
    root: Value,
    storage: CredentialStorage,
    auth_source: String,
) -> Result<ClaudeCredentials, ProviderError> {
    let oauth = oauth_object(&root);
    let access_token = first_string(&[oauth, root.as_object()], &["accessToken", "access_token"])
        .ok_or_else(|| {
        ProviderError::parse(provider, "credential payload missing accessToken")
    })?;
    let refresh_token = first_string(
        &[oauth, root.as_object()],
        &["refreshToken", "refresh_token"],
    );
    let plan_hint = first_string(
        &[oauth, root.as_object()],
        &[
            "subscriptionType",
            "subscription_type",
            "plan",
            "plan_type",
            "rateLimitTier",
        ],
    );

    Ok(ClaudeCredentials {
        access_token,
        refresh_token,
        auth_source,
        plan_hint,
        storage,
    })
}

fn parse_credentials_json(provider: ProviderKind, raw: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(raw).map_err(|error| {
        ProviderError::parse(provider, format!("invalid credential JSON: {error}"))
    })
}

fn oauth_object(root: &Value) -> Option<&Map<String, Value>> {
    root.get("claudeAiOauth")
        .and_then(Value::as_object)
        .or_else(|| root.as_object())
}

impl ClaudeCredentials {
    fn persist(&mut self, provider: ProviderKind) -> Result<(), ProviderError> {
        match &mut self.storage {
            CredentialStorage::Env => Ok(()),
            CredentialStorage::File { path, root } => {
                update_credential_value(root, &self.access_token, self.refresh_token.as_deref());
                let serialized = serde_json::to_string_pretty(root).map_err(|error| {
                    ProviderError::parse(
                        provider,
                        format!("failed to serialize refreshed credentials: {error}"),
                    )
                })?;
                fs::write(&*path, serialized).map_err(|error| {
                    ProviderError::io(
                        provider,
                        format!(
                            "failed to write refreshed credentials to {}: {error}",
                            path.display()
                        ),
                    )
                })
            }
            #[cfg(target_os = "macos")]
            CredentialStorage::Keychain { service, root } => {
                update_credential_value(root, &self.access_token, self.refresh_token.as_deref());
                let serialized = serde_json::to_string(root).map_err(|error| {
                    ProviderError::parse(
                        provider,
                        format!("failed to serialize refreshed keychain credentials: {error}"),
                    )
                })?;
                let status = Command::new("security")
                    .args([
                        "add-generic-password",
                        "-U",
                        "-s",
                        service.as_str(),
                        "-w",
                        serialized.as_str(),
                    ])
                    .status()
                    .map_err(|error| {
                        ProviderError::io(
                            provider,
                            format!("failed to execute security add-generic-password: {error}"),
                        )
                    })?;

                if status.success() {
                    Ok(())
                } else {
                    Err(ProviderError::io(
                        provider,
                        format!("security add-generic-password failed while updating {service}"),
                    ))
                }
            }
        }
    }
}

fn update_credential_value(root: &mut Value, access_token: &str, refresh_token: Option<&str>) {
    if let Some(target) = root.get_mut("claudeAiOauth").and_then(Value::as_object_mut) {
        upsert_string_field(target, &["accessToken", "access_token"], access_token);
        if let Some(refresh_token) = refresh_token {
            upsert_string_field(target, &["refreshToken", "refresh_token"], refresh_token);
        }
        return;
    }

    if let Some(target) = root.as_object_mut() {
        upsert_string_field(target, &["accessToken", "access_token"], access_token);
        if let Some(refresh_token) = refresh_token {
            upsert_string_field(target, &["refreshToken", "refresh_token"], refresh_token);
        }
    }
}

fn upsert_string_field(target: &mut Map<String, Value>, keys: &[&str], value: &str) {
    if let Some(existing_key) = keys.iter().find(|key| target.contains_key(**key)) {
        target.insert(
            (*existing_key).to_string(),
            Value::String(value.to_string()),
        );
    } else if let Some(default_key) = keys.first() {
        target.insert((*default_key).to_string(), Value::String(value.to_string()));
    }
}

fn first_string(objects: &[Option<&Map<String, Value>>], keys: &[&str]) -> Option<String> {
    for object in objects.iter().flatten() {
        for key in keys {
            if let Some(value) = object.get(*key).and_then(Value::as_str)
                && !value.is_empty()
            {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub fn take_f64(provider: ProviderKind, value: &Value, field: &str) -> Result<f64, ProviderError> {
    match value {
        Value::Number(number) => number.as_f64().ok_or_else(|| {
            ProviderError::parse(provider, format!("{field} must be a finite number"))
        }),
        Value::String(raw) => raw.parse::<f64>().map_err(|error| {
            ProviderError::parse(provider, format!("{field} must be a number: {error}"))
        }),
        _ => Err(ProviderError::parse(
            provider,
            format!("{field} must be a number"),
        )),
    }
}

pub fn parse_timestamp(
    provider: ProviderKind,
    value: &Value,
    field: &str,
) -> Result<DateTime<Utc>, ProviderError> {
    match value {
        Value::Number(number) => {
            if let Some(seconds) = number.as_i64() {
                timestamp_from_seconds(provider, seconds, field)
            } else if let Some(float_seconds) = number.as_f64() {
                timestamp_from_float(provider, float_seconds, field)
            } else {
                Err(ProviderError::parse(
                    provider,
                    format!("{field} must be a valid timestamp"),
                ))
            }
        }
        Value::String(raw) => {
            if let Ok(timestamp) = DateTime::parse_from_rfc3339(raw) {
                Ok(timestamp.with_timezone(&Utc))
            } else if let Ok(seconds) = raw.parse::<i64>() {
                timestamp_from_seconds(provider, seconds, field)
            } else if let Ok(float_seconds) = raw.parse::<f64>() {
                timestamp_from_float(provider, float_seconds, field)
            } else {
                Err(ProviderError::parse(
                    provider,
                    format!("{field} must be RFC3339 or epoch seconds"),
                ))
            }
        }
        _ => Err(ProviderError::parse(
            provider,
            format!("{field} must be RFC3339 or epoch seconds"),
        )),
    }
}

fn timestamp_from_seconds(
    provider: ProviderKind,
    seconds: i64,
    field: &str,
) -> Result<DateTime<Utc>, ProviderError> {
    Utc.timestamp_opt(seconds, 0).single().ok_or_else(|| {
        ProviderError::parse(
            provider,
            format!("{field} is outside the valid timestamp range"),
        )
    })
}

fn timestamp_from_float(
    provider: ProviderKind,
    seconds: f64,
    field: &str,
) -> Result<DateTime<Utc>, ProviderError> {
    let whole_seconds = seconds.trunc() as i64;
    let nanos = ((seconds.fract().abs()) * 1_000_000_000_f64).round() as u32;
    Utc.timestamp_opt(whole_seconds, nanos)
        .single()
        .ok_or_else(|| {
            ProviderError::parse(
                provider,
                format!("{field} is outside the valid timestamp range"),
            )
        })
}

fn extract_error_message(body: &str, fallback: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|payload| {
            payload
                .get("error_description")
                .and_then(Value::as_str)
                .or_else(|| payload.get("error").and_then(Value::as_str))
                .or_else(|| payload.get("message").and_then(Value::as_str))
                .or_else(|| {
                    payload
                        .get("error")
                        .and_then(Value::as_object)
                        .and_then(|object| object.get("message"))
                        .and_then(Value::as_str)
                })
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

#[cfg(not(target_os = "macos"))]
fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must be set on supported platforms")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_epoch_timestamp() {
        let parsed = parse_timestamp(
            ProviderKind::Codex,
            &Value::from(1_777_534_802_i64),
            "resetsAt",
        )
        .expect("epoch timestamp should parse");

        assert_eq!(parsed.to_rfc3339(), "2026-04-30T07:40:02+00:00");
    }

    #[test]
    fn parses_iso8601_timestamp() {
        let parsed = parse_timestamp(
            ProviderKind::ClaudeCode,
            &Value::from("2026-02-08T04:59:59.000000+00:00"),
            "five_hour.resets_at",
        )
        .expect("RFC3339 timestamp should parse");

        assert_eq!(parsed.to_rfc3339(), "2026-02-08T04:59:59+00:00");
    }

    #[test]
    fn reads_linux_credentials_fixture() {
        let fixture = r#"{
          "claudeAiOauth": {
            "accessToken": "access-token",
            "refreshToken": "refresh-token",
            "subscriptionType": "team"
          },
          "untouched": true
        }"#;

        let root = parse_credentials_json(ProviderKind::ClaudeCode, fixture)
            .expect("fixture should parse");
        let credentials = build_credentials(
            ProviderKind::ClaudeCode,
            root.clone(),
            CredentialStorage::File {
                path: PathBuf::from("/tmp/.credentials.json"),
                root,
            },
            "~/.claude/.credentials.json".to_string(),
        )
        .expect("fixture should build credentials");

        assert_eq!(credentials.access_token, "access-token");
        assert_eq!(credentials.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(credentials.plan_hint.as_deref(), Some("team"));
    }

    #[test]
    fn updates_refresh_tokens_without_losing_unknown_fields() {
        let mut root = serde_json::from_str::<Value>(
            r#"{
              "claudeAiOauth": {
                "accessToken": "old-access",
                "refreshToken": "old-refresh",
                "customField": "keep-me"
              },
              "topLevel": 42
            }"#,
        )
        .expect("fixture should parse");

        update_credential_value(&mut root, "new-access", Some("new-refresh"));

        assert_eq!(
            root["claudeAiOauth"]["accessToken"].as_str(),
            Some("new-access")
        );
        assert_eq!(
            root["claudeAiOauth"]["refreshToken"].as_str(),
            Some("new-refresh")
        );
        assert_eq!(
            root["claudeAiOauth"]["customField"].as_str(),
            Some("keep-me")
        );
        assert_eq!(root["topLevel"].as_i64(), Some(42));
    }

    #[test]
    fn parses_keychain_json_fixture() {
        let fixture = r#"{
          "claudeAiOauth": {
            "accessToken": "keychain-access",
            "refreshToken": "keychain-refresh",
            "expiresAt": 1772984739060,
            "scopes": ["user:profile"],
            "subscriptionType": "pro"
          }
        }"#;

        let root = parse_credentials_json(ProviderKind::ClaudeCode, fixture)
            .expect("fixture should parse");
        let credentials = build_credentials(
            ProviderKind::ClaudeCode,
            root.clone(),
            CredentialStorage::Env,
            "macos-keychain:Claude Code-credentials".to_string(),
        )
        .expect("fixture should build credentials");

        assert_eq!(credentials.access_token, "keychain-access");
        assert_eq!(
            credentials.refresh_token.as_deref(),
            Some("keychain-refresh")
        );
        assert_eq!(credentials.plan_hint.as_deref(), Some("pro"));
    }

    #[test]
    fn parses_refresh_payload() {
        let payload = serde_json::from_str::<Value>(
            r#"{
              "access_token": "new-access",
              "refresh_token": "new-refresh"
            }"#,
        )
        .expect("fixture should parse");
        let object = payload.as_object().expect("payload should be an object");

        let access_token = first_string(&[Some(object)], &["access_token", "accessToken"]);
        let refresh_token = first_string(&[Some(object)], &["refresh_token", "refreshToken"]);

        assert_eq!(access_token.as_deref(), Some("new-access"));
        assert_eq!(refresh_token.as_deref(), Some("new-refresh"));
    }
}
