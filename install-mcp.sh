#!/usr/bin/env bash
# ffs MCP installer — downloads ffs-mcp and auto-registers it with every
# supported AI coding assistant. Idempotent: re-runs merge into existing
# configs without overwriting unrelated entries.
#
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install-mcp.sh | bash
#   ... | bash -s -- --providers cursor,opencode --dry-run
#   ... | bash -s -- --skip-binary             # only register, do not download
#   ... | bash -s -- --uninstall               # remove ffs from every config
#
# Flags:
#   --dest <dir>          binary location (default ~/.local/bin)
#   --version <tag>       pin to a specific release tag (default: latest)
#   --name <id>           server name in config files (default: ffs)
#   --providers <list>    comma-separated subset: claude,codex,cursor,cline,opencode,continue
#                         default = "all" = auto-detect and register every supported provider
#   --skip-binary         skip download; assume ffs-mcp is already on PATH
#   --skip-mcp            skip registration; only install/update the binary
#   --dry-run             print what would be written without modifying any file
#   --uninstall           remove ffs from every provider config (binary kept)
#   --quiet, -q           reduce output to errors
#   -h, --help            show this help and exit

set -eo pipefail

REPO="quangdang46/fast_file_search"
BINARY_NAME="ffs-mcp"
INSTALL_DIR="${FFS_MCP_INSTALL_DIR:-$HOME/.local/bin}"
SERVER_NAME="ffs"
PROVIDERS_RAW="all"
SKIP_BINARY=0
SKIP_MCP=0
DRY_RUN=0
UNINSTALL=0
QUIET=0
VERSION=""

while [ $# -gt 0 ]; do
    case "$1" in
        --dest) INSTALL_DIR="$2"; shift 2 ;;
        --version) VERSION="$2"; shift 2 ;;
        --name) SERVER_NAME="$2"; shift 2 ;;
        --providers) PROVIDERS_RAW="$2"; shift 2 ;;
        --skip-binary) SKIP_BINARY=1; shift ;;
        --skip-mcp) SKIP_MCP=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        --quiet|-q) QUIET=1; shift ;;
        -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown flag: $1" >&2; exit 2 ;;
    esac
done

info()    { [ "$QUIET" -eq 1 ] || printf '\033[1;34m%s\033[0m\n' "$*"; }
success() { [ "$QUIET" -eq 1 ] || printf '\033[1;38;5;208m%s\033[0m\n' "$*"; }
warn()    { printf '\033[1;33m%s\033[0m\n' "$*" >&2; }
error()   { printf '\033[1;31mError: %s\033[0m\n' "$*" >&2; exit 1; }

require_jq() {
    if ! command -v jq >/dev/null 2>&1; then
        error "jq is required for auto-registering MCP. Install via apt-get install jq / brew install jq, or re-run with --skip-mcp."
    fi
}

detect_platform() {
    local os arch target
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)
            case "$arch" in
                x86_64)  target="x86_64-unknown-linux-musl" ;;
                aarch64|arm64) target="aarch64-unknown-linux-musl" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac ;;
        Darwin)
            case "$arch" in
                x86_64)  target="x86_64-apple-darwin" ;;
                aarch64|arm64) target="aarch64-apple-darwin" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac ;;
        MINGW*|MSYS*|CYGWIN*)
            case "$arch" in
                x86_64)  target="x86_64-pc-windows-msvc" ;;
                aarch64|arm64) target="aarch64-pc-windows-msvc" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac ;;
        *) error "Unsupported OS: $os" ;;
    esac
    echo "$target"
}

get_latest_release_tag() {
    local target="$1"
    local releases_json
    releases_json=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases") \
        || error "Failed to fetch releases from https://github.com/${REPO}/releases"
    local tag
    tag=$(echo "$releases_json" \
        | grep -oE '"(tag_name|name)": *"[^"]*"' \
        | awk -v target="ffs-mcp-${target}" '
            /"tag_name":/ { gsub(/.*": *"|"/, ""); current_tag = $0; next }
            /"name":/ && index($0, target) { print current_tag; exit }
        ')
    if [ -z "$tag" ]; then
        error "No release found containing ffs-mcp binaries for ${target}."
    fi
    echo "$tag"
}

download_binary() {
    local target="$1" tag="$2" ext=""
    case "$target" in *windows*) ext=".exe" ;; esac
    local filename="${BINARY_NAME}-${target}${ext}"
    local url="https://github.com/${REPO}/releases/download/${tag}/${filename}"
    local checksum_url="${url}.sha256"
    info "Downloading ${filename} from release ${tag}..."

    local tmp_dir
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$tmp_dir"' EXIT
    if ! curl -fsSL -o "${tmp_dir}/${filename}" "$url" 2>/dev/null; then
        error "Failed to download binary for your platform: ${url}"
    fi
    if command -v sha256sum >/dev/null 2>&1; then
        if curl -fsSL -o "${tmp_dir}/${filename}.sha256" "$checksum_url" 2>/dev/null; then
            info "Verifying checksum..."
            (cd "$tmp_dir" && sha256sum -c "${filename}.sha256" >/dev/null) \
                || error "Checksum verification failed."
        fi
    fi
    mkdir -p "$INSTALL_DIR"
    mv "${tmp_dir}/${filename}" "${INSTALL_DIR}/${BINARY_NAME}${ext}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}${ext}"
    success "Installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}${ext}"
}

binary_command() {
    if [ -x "${INSTALL_DIR}/${BINARY_NAME}" ]; then
        echo "${INSTALL_DIR}/${BINARY_NAME}"
    else
        echo "${BINARY_NAME}"
    fi
}

# --- per-provider auto-registration ---

# Atomic JSON merge: read source (or default to {}), apply jq filter, write back.
jq_merge() {
    local file="$1"
    local filter="$2"
    require_jq
    mkdir -p "$(dirname "$file")"
    local existing="{}"
    [ -f "$file" ] && existing="$(cat "$file")"
    local merged
    merged=$(echo "$existing" | jq "$filter")
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would write $file:"
        echo "$merged"
    else
        printf '%s\n' "$merged" > "$file"
        success "[mcp] wrote $file"
    fi
}

# Selectively remove a key inside an MCP config file.
jq_unset() {
    local file="$1"
    local filter="$2"
    [ -f "$file" ] || return 0
    require_jq
    local merged
    merged=$(cat "$file" | jq "$filter")
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would remove ${SERVER_NAME} from $file"
    else
        printf '%s\n' "$merged" > "$file"
        success "[mcp] removed ${SERVER_NAME} from $file"
    fi
}

register_claude() {
    command -v claude >/dev/null 2>&1 || { info "claude CLI not detected — skipping Claude Code"; return; }
    local cmd
    cmd=$(binary_command)
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would run: claude mcp add -s user $SERVER_NAME -- $cmd"
        return
    fi
    # Idempotent: remove first then re-add (Anthropic CLI returns non-zero on duplicate).
    claude mcp remove -s user "$SERVER_NAME" >/dev/null 2>&1 || true
    if claude mcp add -s user "$SERVER_NAME" -- "$cmd" >/dev/null 2>&1; then
        success "[mcp] registered with Claude Code (-s user)"
    else
        warn "claude mcp add failed; falling back to direct file edit of ~/.claude.json"
        jq_merge "$HOME/.claude.json" \
            ".mcpServers[\"$SERVER_NAME\"] = {\"type\":\"stdio\",\"command\":\"$cmd\",\"args\":[]}"
    fi
}

unregister_claude() {
    command -v claude >/dev/null 2>&1 || return
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would run: claude mcp remove -s user $SERVER_NAME"
        return
    fi
    claude mcp remove -s user "$SERVER_NAME" >/dev/null 2>&1 && \
        success "[mcp] removed ${SERVER_NAME} from Claude Code"
}

register_codex() {
    command -v codex >/dev/null 2>&1 || { info "codex CLI not detected — skipping Codex"; return; }
    local cmd
    cmd=$(binary_command)
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would run: codex mcp add $SERVER_NAME -- $cmd"
        return
    fi
    codex mcp remove "$SERVER_NAME" >/dev/null 2>&1 || true
    if codex mcp add "$SERVER_NAME" -- "$cmd" >/dev/null 2>&1; then
        success "[mcp] registered with Codex"
    else
        warn "codex mcp add failed; check ~/.codex/config.toml manually"
    fi
}

unregister_codex() {
    command -v codex >/dev/null 2>&1 || return
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would run: codex mcp remove $SERVER_NAME"
        return
    fi
    codex mcp remove "$SERVER_NAME" >/dev/null 2>&1 && \
        success "[mcp] removed ${SERVER_NAME} from Codex"
}

# Cursor: ~/.cursor/mcp.json with { mcpServers: { name: { command, args, type } } }
register_cursor() {
    local cfg="$HOME/.cursor/mcp.json"
    if [ ! -d "$HOME/.cursor" ] && ! command -v cursor >/dev/null 2>&1; then
        info "Cursor not detected — skipping"
        return
    fi
    local cmd
    cmd=$(binary_command)
    jq_merge "$cfg" \
        ".mcpServers = (.mcpServers // {}) | .mcpServers[\"$SERVER_NAME\"] = {\"command\":\"$cmd\",\"args\":[],\"type\":\"stdio\"}"
}

unregister_cursor() {
    jq_unset "$HOME/.cursor/mcp.json" "del(.mcpServers[\"$SERVER_NAME\"])"
}

# Cline (VSCode/Cursor extension): cline_mcp_settings.json in extension storage.
# Path varies; we honour CLINE_MCP_SETTINGS env var if set.
cline_settings_path() {
    if [ -n "${CLINE_MCP_SETTINGS:-}" ]; then
        echo "$CLINE_MCP_SETTINGS"
        return
    fi
    local os
    os="$(uname -s)"
    local base
    case "$os" in
        Darwin) base="$HOME/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings" ;;
        Linux)  base="$HOME/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings" ;;
        MINGW*|MSYS*|CYGWIN*) base="$APPDATA/Code/User/globalStorage/saoudrizwan.claude-dev/settings" ;;
        *) base="$HOME/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings" ;;
    esac
    echo "$base/cline_mcp_settings.json"
}

register_cline() {
    local cfg
    cfg="$(cline_settings_path)"
    # Cline is a VSCode extension — only proceed if its storage dir exists OR
    # the user explicitly opted in via CLINE_MCP_SETTINGS.
    if [ ! -d "$(dirname "$cfg")" ] && [ -z "${CLINE_MCP_SETTINGS:-}" ]; then
        info "Cline storage dir not found — skipping (set CLINE_MCP_SETTINGS to override)"
        return
    fi
    local cmd
    cmd=$(binary_command)
    jq_merge "$cfg" \
        ".mcpServers = (.mcpServers // {}) | .mcpServers[\"$SERVER_NAME\"] = {\"command\":\"$cmd\",\"args\":[],\"transportType\":\"stdio\"}"
}

unregister_cline() {
    local cfg
    cfg="$(cline_settings_path)"
    jq_unset "$cfg" "del(.mcpServers[\"$SERVER_NAME\"])"
}

# OpenCode: ~/.config/opencode/opencode.json with { mcp: { name: { type, command, enabled } } }
register_opencode() {
    if ! command -v opencode >/dev/null 2>&1 && [ ! -d "$HOME/.config/opencode" ]; then
        info "OpenCode not detected — skipping"
        return
    fi
    local cfg="$HOME/.config/opencode/opencode.json"
    local cmd
    cmd=$(binary_command)
    jq_merge "$cfg" \
        ".mcp = (.mcp // {}) | .mcp[\"$SERVER_NAME\"] = {\"type\":\"local\",\"command\":[\"$cmd\"],\"enabled\":true}"
}

unregister_opencode() {
    jq_unset "$HOME/.config/opencode/opencode.json" "del(.mcp[\"$SERVER_NAME\"])"
}

# Continue.dev: ~/.continue/mcpServers/<name>.yaml
register_continue() {
    if [ ! -d "$HOME/.continue" ]; then
        info "Continue not detected — skipping"
        return
    fi
    local dir="$HOME/.continue/mcpServers"
    local file="${dir}/${SERVER_NAME}.yaml"
    local cmd
    cmd=$(binary_command)
    local body
    body=$(cat <<EOF
name: ${SERVER_NAME}
version: 0.0.1
schema: v1
mcpServers:
  - name: ${SERVER_NAME}
    command: ${cmd}
    args: []
EOF
)
    if [ "$DRY_RUN" -eq 1 ]; then
        info "[dry-run] would write $file:"
        echo "$body"
    else
        mkdir -p "$dir"
        printf '%s\n' "$body" > "$file"
        success "[mcp] wrote $file"
    fi
}

unregister_continue() {
    local file="$HOME/.continue/mcpServers/${SERVER_NAME}.yaml"
    if [ -f "$file" ]; then
        if [ "$DRY_RUN" -eq 1 ]; then
            info "[dry-run] would delete $file"
        else
            rm -f "$file"
            success "[mcp] deleted $file"
        fi
    fi
}

run_provider() {
    local provider="$1"
    local action="$2"
    case "$provider:$action" in
        claude:register)   register_claude ;;
        claude:remove)     unregister_claude ;;
        codex:register)    register_codex ;;
        codex:remove)      unregister_codex ;;
        cursor:register)   register_cursor ;;
        cursor:remove)     unregister_cursor ;;
        cline:register)    register_cline ;;
        cline:remove)      unregister_cline ;;
        opencode:register) register_opencode ;;
        opencode:remove)   unregister_opencode ;;
        continue:register) register_continue ;;
        continue:remove)   unregister_continue ;;
        *) warn "unknown provider:action $provider:$action" ;;
    esac
}

provider_list() {
    if [ "$PROVIDERS_RAW" = "all" ]; then
        echo "claude codex cursor cline opencode continue"
    else
        echo "$PROVIDERS_RAW" | tr ',' ' '
    fi
}

main() {
    local target tag
    target="$(detect_platform)"

    if [ "$UNINSTALL" -eq 1 ]; then
        info "Unregistering ${SERVER_NAME} from all configured providers..."
        for p in $(provider_list); do run_provider "$p" remove; done
        info "Done. Binary at ${INSTALL_DIR}/${BINARY_NAME} kept; remove it manually if desired."
        return
    fi

    if [ "$SKIP_BINARY" -eq 0 ]; then
        if [ -n "$VERSION" ]; then
            tag="$VERSION"
        else
            tag="$(get_latest_release_tag "$target")"
        fi
        download_binary "$target" "$tag"
    else
        info "Skipping binary install (per --skip-binary)."
    fi

    if [ "$SKIP_MCP" -eq 0 ]; then
        info "Registering ${SERVER_NAME} with MCP providers..."
        for p in $(provider_list); do run_provider "$p" register; done
    else
        info "Skipping MCP registration (per --skip-mcp)."
    fi

    success ""
    success "ffs MCP install complete."
    info "Run 'ffs-mcp --version' to verify. Use --uninstall to roll back the config edits."
}

main
