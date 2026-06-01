---
name: codequota
description: Use when an agent needs to run, explain, test, or automate the `codequota` CLI in this repository, including command usage, JSON response shape, auth-source behavior, exit codes, and provider-specific troubleshooting for Claude Code, Claude Desktop, and Codex.
---

# Codequota

`codequota` is a Rust CLI in this repository that reads existing local auth credentials, calls provider usage APIs, and reports quota usage for:

- `claude-code`
- `claude-desktop`
- `codex`

Use this skill when the task is about:

- running the CLI
- explaining what a response means
- automating against `--json`
- debugging auth-source failures
- changing output or provider behavior

## Quick Start

From the repo root, prefer:

```sh
cargo run --bin codequota --
cargo run --bin codequota -- --json
cargo run --bin codequota -- codex --json
```

If the binary is already built and you need repeated calls:

```sh
./target/debug/codequota
./target/debug/codequota --json
./target/debug/codequota claude-code --json
```

If the CLI is installed globally:

```sh
codequota
codequota usage
codequota codex --json
```

## Commands

The supported commands are:

- `codequota`
- `codequota usage`
- `codequota claude-code`
- `codequota claude-desktop`
- `codequota codex`
- `codequota help`
- `codequota help <topic>`
- `codequota version`

`--json` is supported on:

- top-level `codequota --json`
- `codequota usage --json`
- `codequota claude-code --json`
- `codequota claude-desktop --json`
- `codequota codex --json`

## Output Modes

### Human-readable output

Human-readable output is one line per provider. The shape is:

```text
<provider>  <status>  plan=<plan-or-n/a>  5h=<percent-or-n/a> reset=<timestamp-or-n/a>  7d=<percent-or-n/a> reset=<timestamp-or-n/a>  source=<auth-source>
```

On failure, the line ends with `error=<message>` instead of `source=...`.

Example:

```text
codex  ok  plan=team  5h=12% reset=2026-06-01T08:00:00+00:00  7d=3% reset=2026-06-08T08:00:00+00:00  source=macos-keychain:Codex Auth
```

If a window is unavailable, it is rendered as:

```text
5h=n/a reset=n/a
7d=n/a reset=n/a
```

### JSON output

For automation, always prefer `--json`.

- `codequota --json` and `codequota usage --json` return a JSON array of records.
- `codequota <provider> --json` returns one JSON object.

Each record has this shape:

```json
{
  "provider": "codex",
  "status": "ok",
  "auth_source": "macos-keychain:Codex Auth",
  "plan": "team",
  "five_hour": {
    "utilization": 12.0,
    "resets_at": "2026-06-01T08:00:00Z"
  },
  "seven_day": {
    "utilization": 3.0,
    "resets_at": "2026-06-08T08:00:00Z"
  },
  "generated_at": "2026-06-01T04:00:00Z"
}
```

Error records omit success-only fields when absent and include `error`:

```json
{
  "provider": "codex",
  "status": "error",
  "generated_at": "2026-06-01T04:00:00Z",
  "error": "failed to read credentials"
}
```

## JSON Contract

Field meanings:

- `provider`: one of `claude-code`, `claude-desktop`, `codex`
- `status`: one of `ok`, `error`, `unsupported`
- `auth_source`: present on success when the credential source is known
- `plan`: provider-reported plan, or omitted when unavailable
- `five_hour`: optional usage window
- `seven_day`: optional usage window
- `generated_at`: RFC3339 UTC timestamp for when `codequota` produced the record
- `error`: present on error or unsupported records

Window fields:

- `utilization`: numeric percentage already normalized for display and automation
- `resets_at`: RFC3339 timestamp or `null`

Important details:

- `resets_at` may be `null`. Do not assume it is always present.
- A provider may return `ok` even if one window is missing, as long as the provider-specific parser accepted the payload.
- For all-provider JSON output, do not assume every record has the same `status`.

## Exit Codes

Exit code behavior is asymmetric:

- Single-provider commands return `0` only when that provider status is `ok`.
- Single-provider commands return `1` for `error` or `unsupported`.
- All-provider commands return `0` if any provider succeeded.
- All-provider commands return `1` only if every provider failed or was unsupported.

This matters for automation:

- Use `codequota <provider> --json` for strict health checks.
- Use `codequota --json` when partial success is acceptable.

## Auth Sources

`codequota` does not perform interactive login. It reuses credentials that already exist or were injected by environment variables.

### Claude Code

Credential lookup order is effectively:

- `CLAUDE_CODE_OAUTH_TOKEN`
- macOS Keychain `Claude Code-credentials`
- Linux credential file under `~/.claude/.credentials.json` or `$CLAUDE_CONFIG_DIR/.credentials.json`

Claude Code may refresh expired access tokens if a refresh token is available.

### Claude Desktop

- Supported on macOS
- Unsupported on Linux
- Uses saved desktop credentials on macOS

### Codex

Credential sources are:

- `CODEX_ACCESS_TOKEN`
- `CODEQUOTA_CODEX_AUTH_FILE`
- macOS Keychain `Codex Auth` when Codex is configured for `keyring` or `auto`
- `$CODEX_HOME/auth.json`
- `~/.codex/auth.json`

Optional account-id overrides:

- `CODEX_ACCOUNT_ID`
- `CHATGPT_ACCOUNT_ID`

Codex usage requests send:

- `Authorization: Bearer <access-token>`
- `ChatGPT-Account-Id: <account-id>` when available

## Troubleshooting

### `status=error`

Read the `error` field literally. Common cases:

- missing credential file
- missing keychain entry
- unauthorized or forbidden API response
- provider returned an unexpected JSON shape

### Codex works in `codex login status` but not in `codequota`

Check:

- whether `CODEX_HOME` is set
- whether Codex is using `cli_auth_credentials_store = "keyring"` or `auto`
- whether the installed `codequota` binary is new enough to read the current storage mode

### `claude-desktop` fails on Linux

That is expected. It should surface as `unsupported`.

### Human output is hard to parse

Do not parse the human format. Re-run with `--json`.

## Repo-Specific Debugging Notes

For tests and mocked integration work, these env vars are useful:

- `CODEQUOTA_ANTHROPIC_USAGE_URL`
- `CODEQUOTA_CODEX_USAGE_URL`
- `CODEQUOTA_CODEX_AUTH_FILE`

Use them to point the CLI at a mock server or fixture instead of real upstream endpoints.

## Safe Agent Behavior

When writing examples or explanations:

- never include host-specific usernames, home-directory expansions with real names, machine names, or local filesystem paths copied from a live machine
- use placeholders such as `/path/to/auth.json` or `$CODEX_HOME/auth.json`
- do not expose access tokens, refresh tokens, or keychain payloads
- prefer exact timestamps returned by the CLI over relative wording like "today" or "later"

When changing CLI behavior:

1. run `cargo test`
2. run at least one matching CLI command with `--json`
3. if output shape changed, update this skill and `README.md`
