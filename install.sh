#!/usr/bin/env bash
# ffs installer — downloads the right native binary from GitHub Releases
# and atomically installs it to $DEST (default ~/.local/bin).
#
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh | bash -s -- --easy-mode
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh | bash -s -- --version v0.1.0
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh | bash -s -- --uninstall
#
# Flags:
#   --dest <dir>            install location (default ~/.local/bin)
#   --system                install to /usr/local/bin (needs sudo)
#   --version <tag>         pin to a specific release tag (e.g. v0.1.0); default: latest
#   --easy-mode             auto-append PATH export to ~/.bashrc and ~/.zshrc
#   --verify                run `ffs --version` after install as a self-test
#   --from-source           skip download, build with cargo (slow)
#   --mcp                   also download ffs-mcp and auto-register with every detected
#                           AI assistant (Claude Code, Codex, Cursor, Cline, OpenCode, Continue)
#   --mcp-providers <list>  restrict --mcp to comma-separated subset of providers
#   --mcp-dry-run           run --mcp registration in dry-run mode (no files modified)
#   --quiet, -q             silence informational output
#   --uninstall             remove the binary and the easy-mode PATH lines
#   -h, --help              show this help and exit

set -euo pipefail
umask 022

# === Config ===
BINARY_NAME="ffs"
OWNER="quangdang46"
REPO="fast_file_search"
DEST="${DEST:-$HOME/.local/bin}"
VERSION="${VERSION:-}"
QUIET=0
EASY=0
VERIFY=0
FROM_SOURCE=0
UNINSTALL=0
MCP=0
MCP_PROVIDERS="all"
MCP_DRY_RUN=0
MAX_RETRIES=3
DOWNLOAD_TIMEOUT=120
LOCK_DIR="/tmp/${BINARY_NAME}-install.lock.d"
TMP=""

# === Logging ===
log_info()    { [ "$QUIET" -eq 1 ] && return 0; printf '[%s] %s\n' "$BINARY_NAME" "$*" >&2; }
log_warn()    { printf '[%s] WARN: %s\n' "$BINARY_NAME" "$*" >&2; }
log_success() { [ "$QUIET" -eq 1 ] && return 0; printf '\xE2\x9C\x93 %s\n' "$*" >&2; }
die()         { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

# === Cleanup & lock ===
cleanup() { rm -rf "$TMP" "$LOCK_DIR" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
acquire_lock() {
    if mkdir "$LOCK_DIR" 2>/dev/null; then
        echo $$ > "$LOCK_DIR/pid"
        return 0
    fi
    die "Another install is running. If stuck: rm -rf $LOCK_DIR"
}

# === Args ===
while [ $# -gt 0 ]; do
    case "$1" in
        --dest)          DEST="$2";          shift 2;;
        --dest=*)        DEST="${1#*=}";     shift;;
        --version)       VERSION="$2";       shift 2;;
        --version=*)     VERSION="${1#*=}";  shift;;
        --system)        DEST="/usr/local/bin"; shift;;
        --easy-mode)     EASY=1;             shift;;
        --verify)        VERIFY=1;           shift;;
        --from-source)   FROM_SOURCE=1;      shift;;
        --quiet|-q)      QUIET=1;            shift;;
        --uninstall)     UNINSTALL=1;        shift;;
        --mcp)           MCP=1;              shift;;
        --mcp-providers) MCP=1; MCP_PROVIDERS="$2"; shift 2;;
        --mcp-providers=*) MCP=1; MCP_PROVIDERS="${1#*=}"; shift;;
        --mcp-dry-run)   MCP=1; MCP_DRY_RUN=1;  shift;;
        -h|--help)       usage;;
        *) log_warn "Unknown flag: $1"; shift;;
    esac
done

# === Uninstall path ===
do_uninstall() {
    local target="$DEST/$BINARY_NAME"
    if [ -f "$target" ]; then
        rm -f "$target"
        log_success "Removed $target"
    else
        log_warn "Not found at $target — nothing to remove"
    fi
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        [ -f "$rc" ] || continue
        sed -i.bak "/${BINARY_NAME} installer/d" "$rc" 2>/dev/null \
            || sed -i '' "/${BINARY_NAME} installer/d" "$rc" 2>/dev/null \
            || true
        rm -f "$rc.bak" 2>/dev/null || true
    done
    log_success "Uninstalled"
    exit 0
}
[ "$UNINSTALL" -eq 1 ] && do_uninstall

# === Platform → asset suffix ===
# The release.yaml uploads bare ffs binaries named ffs-<rust-triple>{.exe}.
# We prefer musl on Linux for portability across glibc versions.
detect_target() {
    local os arch
    case "$(uname -s)" in
        Linux*)               os="linux";;
        Darwin*)              os="darwin";;
        MINGW*|MSYS*|CYGWIN*) os="windows";;
        *) die "Unsupported OS: $(uname -s)";;
    esac
    case "$(uname -m)" in
        x86_64|amd64)   arch="x86_64";;
        aarch64|arm64)  arch="aarch64";;
        *) die "Unsupported arch: $(uname -m)";;
    esac

    case "${os}_${arch}" in
        linux_x86_64)    echo "x86_64-unknown-linux-musl";;
        linux_aarch64)   echo "aarch64-unknown-linux-musl";;
        darwin_x86_64)   echo "x86_64-apple-darwin";;
        darwin_aarch64)  echo "aarch64-apple-darwin";;
        windows_x86_64)  echo "x86_64-pc-windows-msvc";;
        windows_aarch64) echo "aarch64-pc-windows-msvc";;
        *) die "Unsupported platform: ${os}_${arch}";;
    esac
}

is_windows() { [[ "$(uname -s)" =~ ^(MINGW|MSYS|CYGWIN) ]]; }

# === Version resolution ===
resolve_version() {
    [ -n "$VERSION" ] && { log_info "Using pinned version: $VERSION"; return 0; }

    # 1st try: GitHub API
    VERSION=$(curl -fsSL --connect-timeout 10 --max-time 30 \
        -H "Accept: application/vnd.github.v3+json" \
        "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" 2>/dev/null \
        | grep '"tag_name":' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/') || true

    # 2nd fallback: redirect trick (works without API rate limit)
    if ! [[ "${VERSION:-}" =~ ^v[0-9] ]]; then
        VERSION=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
            "https://github.com/${OWNER}/${REPO}/releases/latest" 2>/dev/null \
            | sed -E 's|.*/tag/||') || true
    fi

    [[ "${VERSION:-}" =~ ^v[0-9] ]] || die "Could not resolve latest version"
    log_info "Latest version: $VERSION"
}

# === Download with retry/resume ===
download_file() {
    local url="$1" dest="$2"
    local partial="${dest}.part"
    local attempt=0
    local progress="-sS"
    [ "$QUIET" -eq 0 ] && [ -t 2 ] && progress="--progress-bar"

    while [ $attempt -lt $MAX_RETRIES ]; do
        attempt=$((attempt + 1))
        local resume=""
        [ -s "$partial" ] && resume="--continue-at -"
        # shellcheck disable=SC2086
        if curl -fL --connect-timeout 30 --max-time "$DOWNLOAD_TIMEOUT" \
                $progress --retry 2 $resume \
                -o "$partial" "$url"; then
            mv -f "$partial" "$dest"
            return 0
        fi
        [ $attempt -lt $MAX_RETRIES ] && { log_warn "Retrying in 3s ($attempt/$MAX_RETRIES)..."; sleep 3; }
    done
    rm -f "$partial" 2>/dev/null || true
    return 1
}

# === Atomic install ===
install_binary_atomic() {
    local src="$1" dst="$2"
    local tmp="${dst}.tmp.$$"
    install -m 0755 "$src" "$tmp" || { rm -f "$tmp"; die "install(1) failed"; }
    mv -f "$tmp" "$dst" || { rm -f "$tmp"; die "Failed to move into place"; }
}

# === sha256 verify ===
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo ""
    fi
}

verify_checksum() {
    local file="$1" checksum_file="$2"
    [ -f "$checksum_file" ] || return 0  # no sidecar = skip
    local expected actual
    expected=$(awk '{print $1}' "$checksum_file")
    actual=$(sha256_of "$file")
    if [ -z "$actual" ]; then
        log_warn "No sha256 tool available; skipping checksum verification"
        return 0
    fi
    [ "$expected" = "$actual" ] || die "Checksum mismatch (expected $expected, got $actual)"
    log_info "Checksum verified"
}

# === PATH ===
maybe_add_path() {
    case ":$PATH:" in *":$DEST:"*) return 0;; esac
    if [ "$EASY" -eq 1 ]; then
        local appended=0
        for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
            [ -f "$rc" ] && [ -w "$rc" ] || continue
            grep -qF "$DEST" "$rc" && continue
            printf '\nexport PATH="%s:$PATH"  # %s installer\n' "$DEST" "$BINARY_NAME" >> "$rc"
            appended=1
            log_info "Added PATH export to $rc"
        done
        if [ $appended -eq 1 ]; then
            log_warn "Restart your shell or run: export PATH=\"$DEST:\$PATH\""
        fi
    else
        log_warn "Not on PATH. Add to your shell rc, or rerun with --easy-mode:"
        log_warn "  export PATH=\"$DEST:\$PATH\""
    fi
}

# === From-source build ===
build_from_source() {
    command -v cargo >/dev/null 2>&1 \
        || die "cargo not found. Install Rust: https://rustup.rs"
    command -v git >/dev/null 2>&1 \
        || die "git not found"

    log_info "Cloning ${OWNER}/${REPO}..."
    git clone --depth 1 "https://github.com/${OWNER}/${REPO}.git" "$TMP/src"

    log_info "Building (this may take several minutes)..."
    (cd "$TMP/src" && CARGO_TARGET_DIR="$TMP/target" \
        cargo build --release --locked -p ffs-cli)

    install_binary_atomic "$TMP/target/release/$BINARY_NAME" "$DEST/$BINARY_NAME"
    log_success "Built from source: $DEST/$BINARY_NAME"
}

# === Main ===
main() {
    acquire_lock
    TMP=$(mktemp -d)
    mkdir -p "$DEST"

    if [ "$FROM_SOURCE" -eq 1 ]; then
        build_from_source
    else
        local target asset url
        target=$(detect_target)
        log_info "Detected target: $target | Dest: $DEST"

        resolve_version

        if is_windows; then
            asset="ffs-${target}.exe"
        else
            asset="ffs-${target}"
        fi
        url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${asset}"

        log_info "Downloading $asset..."
        if download_file "$url" "$TMP/$asset"; then
            # Try sha256 sidecar; non-fatal if missing.
            download_file "${url}.sha256" "$TMP/${asset}.sha256" 2>/dev/null \
                && verify_checksum "$TMP/$asset" "$TMP/${asset}.sha256" \
                || log_warn "No sha256 sidecar at ${url}.sha256 — skipping verification"

            local install_name="$BINARY_NAME"
            is_windows && install_name="${BINARY_NAME}.exe"
            install_binary_atomic "$TMP/$asset" "$DEST/$install_name"
            log_success "Installed $DEST/$install_name"
        else
            log_warn "Binary download failed — falling back to from-source build"
            build_from_source
        fi
    fi

    maybe_add_path

    if [ "$VERIFY" -eq 1 ]; then
        log_info "Running self-test..."
        "$DEST/$BINARY_NAME" --version || die "Self-test failed"
    fi

    if [ "$MCP" -eq 1 ]; then
        log_info "Auto-registering MCP server with AI assistants..."
        local mcp_url="https://raw.githubusercontent.com/${OWNER}/${REPO}/main/install-mcp.sh"
        local mcp_args="--providers $MCP_PROVIDERS"
        [ "$MCP_DRY_RUN" -eq 1 ] && mcp_args="$mcp_args --dry-run"
        if ! curl -fsSL "$mcp_url" | bash -s -- $mcp_args; then
            log_warn "MCP auto-install failed; see install-mcp.sh --help to retry manually."
        fi
    fi

    echo ""
    echo "ffs installed to $DEST/$BINARY_NAME"
    "$DEST/$BINARY_NAME" --version 2>/dev/null || true
    echo ""
    echo "Quick start:"
    echo "  ffs --help"
    echo "  ffs index           # one-time warm-up"
    echo "  ffs find <query>"
    echo "  ffs grep <pattern>"
    echo "  ffs symbol <name>"
}

# curl|bash safety: buffer the whole script before running so a truncated
# pipe can't half-execute the installer.
if [[ "${BASH_SOURCE[0]:-}" == "${0:-}" ]] || [[ -z "${BASH_SOURCE[0]:-}" ]]; then
    { main "$@"; }
fi
