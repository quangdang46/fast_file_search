#!/bin/bash
set -e

VERSION="$1"
if [ -z "$VERSION" ] || ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+'; then
  echo "usage: $0 <semver>  (e.g. $0 0.1.13)"
  exit 1
fi

FILES=(
  Cargo.toml
  crates/ffs-c/Cargo.toml
  crates/ffs-core/Cargo.toml
  crates/ffs-mcp/Cargo.toml
  crates/ffs-query-parser/Cargo.toml
  crates/ffs-grep/Cargo.toml
  crates/ffs-symbol/Cargo.toml
  crates/ffs-budget/Cargo.toml
  crates/ffs-engine/Cargo.toml
  crates/ffs-cli/Cargo.toml
)

# macOS BSD sed needs -i '', GNU sed needs -i (no arg)
sed_i() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

for file in "${FILES[@]}"; do
  [ -f "$file" ] || continue

  # Update [package] version
  sed_i "s/^version = \".*\"/version = \"$VERSION\"/" "$file"

  # Update workspace dependency versions with path references
  sed_i "s/version = \"[0-9]*\.[0-9]*\.[0-9]*\", path/version = \"$VERSION\", path/g" "$file"
done

echo "set-version: all Cargo.toml files updated to $VERSION"
