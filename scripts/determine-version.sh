#!/bin/bash
set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Determine if this is a release.
# Tag push (`refs/tags/v*`) is the normal path. A `release` event also counts
# (out-of-band `gh release create` / UI publishes the tag without a push event —
# that is how v0.1.15 shipped with zero assets; see #75).
IS_RELEASE="false"
if [[ "$GITHUB_REF" == refs/tags/v* || "$GITHUB_EVENT_NAME" == "release" ]]; then
    IS_RELEASE="true"
fi

if [[ "$IS_RELEASE" == "true" ]]; then
    # The git tag is the source of truth for a release. Reading from
    # Cargo.toml instead caused assets to be uploaded to a stale tag (#38).
    if [[ "$GITHUB_REF" == refs/tags/v* ]]; then
        VERSION="${GITHUB_REF#refs/tags/v}"
    elif [[ -n "${GITHUB_REF_NAME:-}" ]]; then
        # release events still set GITHUB_REF to refs/tags/<tag>.
        VERSION="${GITHUB_REF_NAME#v}"
    else
        VERSION=$(grep -E '^version = ' "$REPO_ROOT/crates/ffs-cli/Cargo.toml" | head -1 | sed 's/.*= *"\([^"]*\)".*/\1/')
    fi
else
    # Nightly / branch build: fall back to the ffs-cli Cargo.toml version.
    VERSION=$(grep -E '^version = ' "$REPO_ROOT/crates/ffs-cli/Cargo.toml" | head -1 | sed 's/.*= *"\([^"]*\)".*/\1/')
fi

# Output
echo "version=$VERSION"
echo "npm_tag=latest"
echo "is_release=$IS_RELEASE"

# GitHub output
if [[ -n "$GITHUB_OUTPUT" ]]; then
    echo "version=$VERSION" >> "$GITHUB_OUTPUT"
    echo "npm_tag=latest" >> "$GITHUB_OUTPUT"
    echo "is_release=$IS_RELEASE" >> "$GITHUB_OUTPUT"
fi
