# codequota

`codequota` is a Rust CLI that reads locally stored OAuth credentials and shows usage limits for:

- `claude-code`
- `claude-desktop`
- `codex`

Default output is human-readable. Add `--json` for machine-readable output.

## Usage

```sh
codequota
codequota usage
codequota --json
codequota claude-code
codequota claude-code --json
codequota claude-desktop
codequota codex
codequota help
codequota version
```

`codequota` with no arguments is equivalent to `codequota usage`.

## Commands

| Command | Description |
| --- | --- |
| `codequota` | Show all providers. |
| `codequota usage` | Show all providers. |
| `codequota claude-code` | Show Claude Code usage. |
| `codequota claude-desktop` | Show Claude Desktop usage. |
| `codequota codex` | Show Codex usage. |
| `codequota help` | Show help text. |
| `codequota version` | Print the CLI version. |

`--json` is supported on `usage` and each provider subcommand.

## Output

Human-readable mode prints one line per provider with:

- `plan`
- `5h` utilization and reset timestamp
- `7d` utilization and reset timestamp
- auth source on success, or an error message on failure

`--json` returns normalized records with:

- `provider`
- `status`
- `auth_source`
- `plan`
- `five_hour`
- `seven_day`
- `generated_at`

## Auth Sources

`codequota` does not perform interactive login. It reads credentials that already exist on the machine.

| Provider | Credential source |
| --- | --- |
| `claude-code` | `CLAUDE_CODE_OAUTH_TOKEN`, macOS Keychain `Claude Code-credentials`, or Linux `~/.claude/.credentials.json` / `$CLAUDE_CONFIG_DIR/.credentials.json` |
| `claude-desktop` | macOS saved desktop credentials only |
| `codex` | `CODEX_ACCESS_TOKEN`, `CODEQUOTA_CODEX_AUTH_FILE`, or `~/.codex/auth.json` |

Claude Code refreshes expired access tokens when a refresh token is available, then writes the rotated token back to the original store.

Recent Codex CLI builds may store ChatGPT login state in keyring instead of `~/.codex/auth.json`. In that case, export `CODEX_ACCESS_TOKEN` before running `codequota`, or point `CODEQUOTA_CODEX_AUTH_FILE` at a compatible JSON auth file.

## Platform Support

| Provider | Linux | macOS |
| --- | --- | --- |
| `claude-code` | Supported | Supported |
| `claude-desktop` | Unsupported | Supported |
| `codex` | Supported | Supported |

On Linux, `codequota claude-desktop` exits with an error. In `codequota` / `codequota usage`, it is shown as an informational unsupported entry.

## Install

Install the latest release into `~/.local/bin`:

```sh
./install.sh
```

Install directly from GitHub:

```sh
curl -fsSL https://raw.githubusercontent.com/igtm/codequota/main/install.sh | sh
```

Install to a different directory:

```sh
./install.sh -b /usr/local/bin
```

Install system-wide with `sudo`:

```sh
curl -fsSL https://raw.githubusercontent.com/igtm/codequota/main/install.sh | sudo sh -s -- -b /usr/local/bin
```

Install a specific version:

```sh
./install.sh -v 0.0.1
```

Environment overrides:

- `CODEQUOTA_REPO` changes the GitHub repo slug used by the installer. Default: `igtm/codequota`.

## Release Assets

Releases publish these archives:

- `codequota-<version>-x86_64-unknown-linux-gnu.tar.gz`
- `codequota-<version>-aarch64-unknown-linux-gnu.tar.gz`
- `codequota-<version>-x86_64-apple-darwin.tar.gz`
- `codequota-<version>-aarch64-apple-darwin.tar.gz`

Each release also includes `codequota-<version>-checksums.txt`.

## Release Automation

- CI runs `cargo fmt --check`, `cargo clippy`, `cargo test`, and `cargo package --locked`.
- The release workflow runs on pushes to `main` and manual dispatch.
- It reads the version from `Cargo.toml`, creates or updates the matching `vX.Y.Z` tag, builds four target archives, and updates the GitHub release assets for that tag.
