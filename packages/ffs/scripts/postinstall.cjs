#!/usr/bin/env node
"use strict";

// Postinstall: download the platform-native `ffs` binary from GitHub releases
// and place it at packages/ffs/bin/ffs (or ffs.exe on Windows).
//
// Skipped silently when:
//   - FFS_SKIP_POSTINSTALL=1 is set,
//   - the binary already exists at the expected path,
//   - the platform is not supported (logs a warning, exits 0 so `npm install`
//     does not fail downstream installs that do not actually invoke `ffs`).

const fs = require("node:fs");
const path = require("node:path");
const https = require("node:https");
const { execSync } = require("node:child_process");

const REPO = "dmtrKovalenko/ffs.nvim";

function targetTriple() {
  const p = process.platform;
  const a = process.arch;
  if (p === "linux") {
    // Prefer musl for portability across glibc versions.
    if (a === "x64") return "x86_64-unknown-linux-musl";
    if (a === "arm64") return "aarch64-unknown-linux-musl";
  }
  if (p === "darwin") {
    if (a === "x64") return "x86_64-apple-darwin";
    if (a === "arm64") return "aarch64-apple-darwin";
  }
  if (p === "win32") {
    if (a === "x64") return "x86_64-pc-windows-msvc";
    if (a === "arm64") return "aarch64-pc-windows-msvc";
  }
  return null;
}

function binaryName() {
  return process.platform === "win32" ? "ffs.exe" : "ffs";
}

function fetch(url, redirects = 5) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { "User-Agent": "ffs-postinstall" } }, (res) => {
        if (
          res.statusCode &&
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          if (redirects <= 0) {
            reject(new Error(`Too many redirects fetching ${url}`));
            return;
          }
          res.resume();
          fetch(res.headers.location, redirects - 1).then(resolve, reject);
          return;
        }
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} fetching ${url}`));
          return;
        }
        const chunks = [];
        res.on("data", (c) => chunks.push(c));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

async function getReleaseTag() {
  // Pin to package.json version. CI publishes nightly builds tagged
  // `nightly-<sha>` and proper releases tagged `v<semver>`. We try
  // `v<version>` first, then fall back to the latest release.
  const pkg = require("../package.json");
  const version = pkg.version;
  try {
    const data = await fetch(
      `https://api.github.com/repos/${REPO}/releases/tags/v${version}`,
    );
    const json = JSON.parse(data.toString("utf8"));
    if (json && json.tag_name) return json.tag_name;
  } catch {
    // fall through
  }
  const data = await fetch(
    `https://api.github.com/repos/${REPO}/releases/latest`,
  );
  const json = JSON.parse(data.toString("utf8"));
  if (!json.tag_name) {
    throw new Error("No release tag found in GitHub API response");
  }
  return json.tag_name;
}

async function main() {
  if (process.env.FFS_SKIP_POSTINSTALL === "1") {
    console.log("ffs: FFS_SKIP_POSTINSTALL=1 — skipping binary download.");
    return;
  }

  const target = targetTriple();
  if (!target) {
    console.warn(
      `ffs: unsupported platform ${process.platform}-${process.arch}; skipping binary download.`,
    );
    return;
  }

  const binDir = path.resolve(__dirname, "..", "bin");
  const binPath = path.join(binDir, binaryName());
  if (fs.existsSync(binPath)) {
    console.log(`ffs: native binary already present at ${binPath}.`);
    return;
  }

  fs.mkdirSync(binDir, { recursive: true });

  const tag = await getReleaseTag();
  const assetName =
    process.platform === "win32"
      ? `ffs-${target}.exe`
      : `ffs-${target}`;
  const url = `https://github.com/${REPO}/releases/download/${tag}/${assetName}`;
  console.log(`ffs: downloading ${url} ...`);

  const buf = await fetch(url);
  fs.writeFileSync(binPath, buf, { mode: 0o755 });

  // On Unix, ensure executable bit even if mode argument was ignored.
  if (process.platform !== "win32") {
    try {
      execSync(`chmod +x "${binPath}"`);
    } catch {
      // best-effort
    }
  }

  console.log(`ffs: installed ${binPath} (release ${tag}).`);
}

main().catch((err) => {
  console.error(`ffs: postinstall failed: ${err.message}`);
  console.error(
    "ffs: you can build from source instead:\n" +
      "  cargo build --release -p ffs-cli\n" +
      "  cp target/release/ffs <somewhere on PATH>",
  );
  // Exit 0 so a network failure during `npm install` does not break
  // unrelated workflows; the wrapper will print a clear error if invoked.
  process.exit(0);
});
