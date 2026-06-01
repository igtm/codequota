use std::fs;

use assert_cmd::Command;
use mockito::{Matcher, Server};
use predicates::prelude::*;
use serial_test::serial;
use tempfile::tempdir;

fn codex_auth_fixture() -> String {
    r#"{
  "auth_mode": "chatgpt",
  "tokens": {
    "id_token": "id-token",
    "access_token": "access-token",
    "refresh_token": "refresh-token",
    "account_id": "account-id"
  },
  "last_refresh": "2026-05-31T11:44:01.432726434Z"
}"#
    .to_string()
}

#[test]
#[serial]
fn default_command_shows_all_providers_and_succeeds_with_partial_success() {
    let tempdir = tempdir().expect("tempdir should be created");
    let auth_path = tempdir.path().join("auth.json");
    fs::write(&auth_path, codex_auth_fixture()).expect("auth fixture should be written");

    let mut server = Server::new();
    let _mock = server
        .mock("GET", "/backend-api/wham/usage")
        .match_header("authorization", "Bearer access-token")
        .match_header("chatgpt-account-id", "account-id")
        .with_status(200)
        .with_body(
            r#"{
  "rateLimits": {
    "planType": "plus",
    "primary": {
      "usedPercent": 9,
      "windowDurationMins": 300,
      "resetsAt": 1777534802
    },
    "secondary": {
      "usedPercent": 19,
      "windowDurationMins": 10080,
      "resetsAt": 1777969707
    }
  }
}"#,
        )
        .create();

    let mut command = Command::cargo_bin("codequota").expect("binary should build");
    command
        .env("CODEQUOTA_CODEX_AUTH_FILE", &auth_path)
        .env(
            "CODEQUOTA_CODEX_USAGE_URL",
            format!("{}/backend-api/wham/usage", server.url()),
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("claude-code  error"))
        .stdout(predicate::str::contains("claude-desktop  unsupported"))
        .stdout(predicate::str::contains("codex  ok"));
}

#[test]
#[serial]
fn help_subcommand_works() {
    let mut command = Command::cargo_bin("codequota").expect("binary should build");
    command
        .arg("help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Show usage limits"));
}

#[test]
#[serial]
fn version_subcommand_works() {
    let mut command = Command::cargo_bin("codequota").expect("binary should build");
    command
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
#[serial]
fn claude_code_json_output_works() {
    let mut server = Server::new();
    let _mock = server
        .mock("GET", "/api/oauth/usage")
        .match_header("authorization", "Bearer claude-access-token")
        .match_header(
            "anthropic-beta",
            Matcher::Exact("oauth-2025-04-20".to_string()),
        )
        .with_status(200)
        .with_body(
            r#"{
  "five_hour": {
    "utilization": 37.0,
    "resets_at": "2026-02-08T04:59:59.000000+00:00"
  },
  "seven_day": {
    "utilization": 26.0,
    "resets_at": "2026-02-12T14:59:59.771647+00:00"
  }
}"#,
        )
        .create();

    let mut command = Command::cargo_bin("codequota").expect("binary should build");
    command
        .args(["claude-code", "--json"])
        .env("CLAUDE_CODE_OAUTH_TOKEN", "claude-access-token")
        .env(
            "CODEQUOTA_ANTHROPIC_USAGE_URL",
            format!("{}/api/oauth/usage", server.url()),
        )
        .assert()
        .success()
        .stdout(predicate::str::contains("\"provider\": \"claude-code\""))
        .stdout(predicate::str::contains("\"status\": \"ok\""));
}

#[test]
#[serial]
fn claude_desktop_is_unsupported_on_linux() {
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::cargo_bin("codequota").expect("binary should build");
        command
            .arg("claude-desktop")
            .assert()
            .failure()
            .stdout(predicate::str::contains("unsupported on Linux"));
    }
}
