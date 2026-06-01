#!/bin/sh

set -eu

BIN_NAME="codequota"
REPO_SLUG="${CODEQUOTA_REPO:-igtm/codequota}"
INSTALL_DIR=""
INSTALL_DIR_SET=0
REQUESTED_VERSION="latest"

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

is_root() {
  [ "$(id -u)" -eq 0 ]
}

fail() {
  printf '%s\n' "error: $*" >&2
  exit 1
}

download() {
  url="$1"
  output="$2"

  if have_cmd curl; then
    curl --fail --location --silent --show-error "$url" --output "$output"
    return
  fi

  if have_cmd wget; then
    wget -qO "$output" "$url"
    return
  fi

  fail "curl or wget is required"
}

default_install_dir() {
  if is_root; then
    printf '%s\n' '/usr/local/bin'
    return
  fi

  printf '%s\n' "${HOME}/.local/bin"
}

normalize_arch() {
  case "$1" in
    x86_64|amd64)
      printf 'x86_64\n'
      ;;
    arm64|aarch64)
      printf 'aarch64\n'
      ;;
    *)
      fail "unsupported architecture: $1"
      ;;
  esac
}

detect_target() {
  os="$(uname -s)"
  arch="$(normalize_arch "$(uname -m)")"

  case "$os" in
    Linux)
      case "$arch" in
        x86_64)
          printf '%s\n' 'x86_64-unknown-linux-gnu'
          ;;
        aarch64)
          printf '%s\n' 'aarch64-unknown-linux-gnu'
          ;;
        *)
          fail "unsupported Linux architecture: $arch"
          ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64)
          printf '%s\n' 'x86_64-apple-darwin'
          ;;
        aarch64)
          printf '%s\n' 'aarch64-apple-darwin'
          ;;
      esac
      ;;
    *)
      fail "unsupported operating system: $os"
      ;;
  esac
}

resolve_version() {
  if [ "$REQUESTED_VERSION" != "latest" ]; then
    printf '%s\n' "${REQUESTED_VERSION#v}"
    return
  fi

  api_url="https://api.github.com/repos/${REPO_SLUG}/releases/latest"
  tmp_json="$1"

  download "$api_url" "$tmp_json"

  version="$(sed -n 's/.*"tag_name":[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' "$tmp_json" | head -n 1)"
  [ -n "$version" ] || fail "unable to resolve the latest release from ${REPO_SLUG}"
  printf '%s\n' "$version"
}

verify_checksum() {
  checksum_file="$1"
  asset_name="$2"
  asset_path="$3"

  if ! [ -f "$checksum_file" ]; then
    fail "checksum file is missing"
  fi

  expected_line="$(awk -v asset="$asset_name" '$2 == asset { print; exit }' "$checksum_file")"
  [ -n "$expected_line" ] || fail "checksum entry not found for ${asset_name}"

  if have_cmd sha256sum; then
    (
      cd "$(dirname "$asset_path")"
      printf '%s\n' "$expected_line" | sha256sum -c -
    )
    return
  fi

  if have_cmd shasum; then
    expected_sum="$(printf '%s\n' "$expected_line" | awk '{print $1}')"
    actual_sum="$(shasum -a 256 "$asset_path" | awk '{print $1}')"
    [ "$expected_sum" = "$actual_sum" ] || fail "checksum mismatch for ${asset_name}"
    return
  fi

  printf '%s\n' "warning: no SHA-256 tool found; skipping checksum verification" >&2
}

extract_binary() {
  archive="$1"
  target_dir="$2"

  tar -xzf "$archive" -C "$target_dir"

  if [ -x "${target_dir}/${BIN_NAME}" ]; then
    printf '%s\n' "${target_dir}/${BIN_NAME}"
    return
  fi

  fail "archive did not contain ${BIN_NAME}"
}

usage() {
  cat <<EOF
Usage: ./install.sh [-b bindir] [-v version]

Options:
  -b DIR   Install ${BIN_NAME} into DIR (default: ~/.local/bin, or /usr/local/bin when run as root)
  -v VER   Install a specific version (default: latest release)
  -h       Show this help text
EOF
}

parse_args() {
  while getopts "b:v:h" opt; do
    case "$opt" in
      b)
        INSTALL_DIR="$OPTARG"
        INSTALL_DIR_SET=1
        ;;
      v)
        REQUESTED_VERSION="$OPTARG"
        ;;
      h)
        usage
        exit 0
        ;;
      *)
        usage >&2
        exit 1
        ;;
    esac
  done

  shift $((OPTIND - 1))
  if [ "$#" -ne 0 ]; then
    usage >&2
    exit 1
  fi
}

main() {
  parse_args "$@"
  if [ "$INSTALL_DIR_SET" -eq 0 ]; then
    INSTALL_DIR="$(default_install_dir)"
  fi
  target="$(detect_target)"
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT INT TERM

  version="$(resolve_version "${tmp_dir}/release.json")"
  asset_name="${BIN_NAME}-${version}-${target}.tar.gz"
  checksum_name="${BIN_NAME}-${version}-checksums.txt"
  base_url="https://github.com/${REPO_SLUG}/releases/download/v${version}"
  archive_path="${tmp_dir}/${asset_name}"
  checksum_path="${tmp_dir}/${checksum_name}"

  printf '%s\n' "Installing ${BIN_NAME} ${version} for ${target}"

  download "${base_url}/${asset_name}" "$archive_path"
  download "${base_url}/${checksum_name}" "$checksum_path"
  verify_checksum "$checksum_path" "$asset_name" "$archive_path"

  binary_path="$(extract_binary "$archive_path" "$tmp_dir")"

  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$binary_path" "${INSTALL_DIR}/${BIN_NAME}"

  printf '%s\n' "Installed ${BIN_NAME} to ${INSTALL_DIR}/${BIN_NAME}"

  case ":$PATH:" in
    *":${INSTALL_DIR}:"*)
      ;;
    *)
      printf '%s\n' "Add ${INSTALL_DIR} to PATH if you want to run ${BIN_NAME} directly."
      ;;
  esac
}

main "$@"
