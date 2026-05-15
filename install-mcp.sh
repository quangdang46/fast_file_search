#!/usr/bin/env bash
# install-mcp.sh — DEPRECATED thin shim.
#
# MCP registration was unified into install.sh. This wrapper translates
# the legacy install-mcp.sh flag set into the equivalent install.sh
# invocation so existing curl URLs and CI scripts keep working. Prefer
# `install.sh --mcp` (or `install.sh --mcp-only` if ffs is already on
# PATH) for new usage.
#
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh | bash -s -- --mcp
#   curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh | bash -s -- --mcp-only --mcp-providers cursor,opencode
#
# Legacy flags supported here (re-mapped to install.sh equivalents):
#   --providers <list>   -> --mcp-providers <list>
#   --name <id>          -> --mcp-name <id>
#   --dest <dir>         -> --dest <dir>
#   --version <tag>      -> --version <tag>
#   --skip-binary        -> --mcp-only
#   --skip-mcp           -> --no-mcp (binary install only; MCP is default-on)
#   --dry-run            -> --mcp-dry-run
#   --uninstall          -> --mcp-uninstall
#   --quiet, -q          -> --quiet
#   -h, --help           -> show this banner

set -eo pipefail

OWNER="quangdang46"
REPO="fast_file_search"
INSTALL_SH_URL="https://raw.githubusercontent.com/${OWNER}/${REPO}/main/install.sh"

print_help() {
    sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# Translate args.
SKIP_BINARY=0
SKIP_MCP=0
ARGS=()

while [ $# -gt 0 ]; do
    case "$1" in
        --providers)    ARGS+=("--mcp-providers" "$2"); shift 2 ;;
        --providers=*)  ARGS+=("--mcp-providers" "${1#*=}"); shift ;;
        --name)         ARGS+=("--mcp-name" "$2"); shift 2 ;;
        --name=*)       ARGS+=("--mcp-name" "${1#*=}"); shift ;;
        --dest)         ARGS+=("--dest" "$2"); shift 2 ;;
        --dest=*)       ARGS+=("--dest" "${1#*=}"); shift ;;
        --version)      ARGS+=("--version" "$2"); shift 2 ;;
        --version=*)    ARGS+=("--version" "${1#*=}"); shift ;;
        --skip-binary)  SKIP_BINARY=1; shift ;;
        --skip-mcp)     SKIP_MCP=1; shift ;;
        --dry-run)      ARGS+=("--mcp-dry-run"); shift ;;
        --uninstall)    ARGS+=("--mcp-uninstall"); shift ;;
        --quiet|-q)     ARGS+=("--quiet"); shift ;;
        -h|--help)      print_help; exit 0 ;;
        *) printf 'Unknown flag: %s\n' "$1" >&2; exit 2 ;;
    esac
done

# Compose the install.sh mode flag.
if [ "$SKIP_MCP" -eq 1 ] && [ "$SKIP_BINARY" -eq 1 ]; then
    echo "install-mcp.sh: --skip-binary --skip-mcp is a no-op" >&2
    exit 2
elif [ "$SKIP_BINARY" -eq 1 ]; then
    ARGS=("--mcp-only" "${ARGS[@]}")
elif [ "$SKIP_MCP" -eq 1 ]; then
    # MCP is default-on in install.sh now — opt out explicitly.
    ARGS=("--no-mcp" "${ARGS[@]}")
else
    # --mcp is a no-op under the new default-on world but kept for back-compat.
    ARGS=("--mcp" "${ARGS[@]}")
fi

printf '[deprecated] install-mcp.sh is now a wrapper around install.sh.\n' >&2
printf '[deprecated] re-run with: install.sh %s\n' "${ARGS[*]}" >&2

# Two execution modes:
# 1) The script is run from a local checkout (./install-mcp.sh) — exec the
#    sibling install.sh directly.
# 2) The script is run via curl|bash — fetch install.sh from the same repo.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)" || script_dir=""
if [ -n "$script_dir" ] && [ -x "$script_dir/install.sh" ]; then
    exec "$script_dir/install.sh" "${ARGS[@]}"
else
    curl -fsSL "$INSTALL_SH_URL" | bash -s -- "${ARGS[@]}"
fi
