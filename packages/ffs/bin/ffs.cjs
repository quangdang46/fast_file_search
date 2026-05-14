#!/usr/bin/env node
"use strict";

// Thin Node shim that locates the platform-native `ffs` binary fetched
// during postinstall and forwards argv/stdio to it.

const { spawnSync } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");

function binaryPath() {
  const exe = process.platform === "win32" ? "ffs.exe" : "ffs";
  return path.join(__dirname, exe);
}

function main() {
  const bin = binaryPath();
  if (!fs.existsSync(bin)) {
    console.error(
      `ffs: native binary not found at ${bin}.\n` +
        "Try reinstalling: npm install -g @ffs-cli/ffs",
    );
    process.exit(1);
  }
  const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    console.error(`ffs: failed to launch ${bin}: ${result.error.message}`);
    process.exit(1);
  }
  process.exit(result.status ?? 1);
}

main();
