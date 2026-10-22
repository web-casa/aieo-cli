#!/usr/bin/env node
// Runs the prebuilt aieo binary from the platform package npm installed
// alongside (@aieo/cli-<platform>-<arch>).
"use strict";
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const pkg = `@aieo/cli-${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "aieo.exe" : "aieo";
let binary;
try {
  binary = path.join(path.dirname(require.resolve(`${pkg}/package.json`)), "bin", exe);
} catch {
  console.error(
    `aieo: no prebuilt binary for ${process.platform}-${process.arch} (${pkg} is not installed).\n` +
      "Reinstall without --no-optional / --omit=optional, or install from https://github.com/web-casa/aieo-cli/releases"
  );
  process.exit(2);
}
const r = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (r.error) {
  console.error(`aieo: ${r.error.message}`);
  process.exit(2);
}
process.exit(r.status ?? 1);
