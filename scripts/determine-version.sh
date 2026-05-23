#!/bin/bash
set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Extract version from ffs-cli Cargo.toml
VERSION=$(grep -E '^version = ' "$REPO_ROOT/crates/ffs-cli/Cargo.toml" | head -1 | sed 's/.*= *"\([^"]*\)".*/\1/')

# Determine if this is a release (tag starts with v)
IS_RELEASE="false"
if [[ "$GITHUB_REF" == refs/tags/v* ]]; then
    IS_RELEASE="true"
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
