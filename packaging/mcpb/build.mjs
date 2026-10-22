// Assemble the MCP Bundle (.mcpb, for Claude Desktop and other MCPB hosts)
// and the MCP Registry entry from release archives:
//   node packaging/mcpb/build.mjs <version> <dir with aieo-v<version>-<target>.tar.gz|.zip> <out dir>
// Writes <out>/aieo-v<version>.mcpb (+ .sha256) and <out>/server.json.
//
// One bundle serves every desktop: a universal macOS binary (lipo; on Linux
// without lipo the arm64 one, for local testing only), Linux x86_64 and
// Windows x64, chosen with platform_overrides. The tool list in the
// manifest is read from `aieo mcp` itself.
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MCPB_CLI = "@anthropic-ai/mcpb@2.1.2";
const REPO = process.env.GITHUB_REPOSITORY || "web-casa/aieo-cli";
const REGISTRY_NAME = `io.github.${REPO.split("/")[0]}/aieo`;

const [version, assetsArg, outArg] = process.argv.slice(2);
if (!version || !assetsArg || !outArg) {
  console.error("usage: node build.mjs <version> <assets dir> <out dir>");
  process.exit(2);
}
const assets = resolve(assetsArg);
const out = resolve(outArg);
const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");

function extract(target, dest, name) {
  const win = target.includes("windows");
  const archive = join(assets, `aieo-v${version}-${target}.${win ? "zip" : "tar.gz"}`);
  if (!existsSync(archive)) throw new Error(`missing ${archive}`);
  const tmp = mkdtempSync(join(tmpdir(), "aieo-mcpb-"));
  if (win) execFileSync("unzip", ["-o", "-j", archive, "aieo.exe", "-d", tmp]);
  else execFileSync("tar", ["-xzf", archive, "-C", tmp, "aieo"]);
  renameSync(join(tmp, win ? "aieo.exe" : "aieo"), join(dest, name));
  rmSync(tmp, { recursive: true, force: true });
  if (!win) chmodSync(join(dest, name), 0o755);
}

function hasLipo() {
  try {
    execFileSync("lipo", ["-info", "/bin/sh"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

/** tools/list from `aieo mcp`, as the manifest's static tool declaration */
function listTools(bin) {
  return new Promise((ok, fail) => {
    const child = spawn(bin, ["mcp", "-C", tmpdir()], { stdio: ["pipe", "pipe", "inherit"] });
    let buf = "";
    const timer = setTimeout(() => {
      child.kill();
      fail(new Error("aieo mcp did not answer tools/list"));
    }, 15000);
    child.stdout.on("data", (d) => {
      buf += d;
      for (let i; (i = buf.indexOf("\n")) >= 0; ) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        const msg = JSON.parse(line);
        if (msg.id === 1) {
          child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
          child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }) + "\n");
        } else if (msg.id === 2) {
          clearTimeout(timer);
          child.stdin.end();
          ok(msg.result.tools.map((t) => ({ name: t.name, description: t.description.replace(/\s+/g, " ").trim() })));
        }
      }
    });
    child.stdin.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "mcpb-build", version } },
      }) + "\n",
    );
  });
}

const dir = join(out, "bundle");
rmSync(dir, { recursive: true, force: true });
mkdirSync(join(dir, "server"), { recursive: true });

extract("x86_64-unknown-linux-musl", join(dir, "server"), "aieo-linux");
extract("x86_64-pc-windows-msvc", join(dir, "server"), "aieo.exe");
if (hasLipo()) {
  const tmp = mkdtempSync(join(tmpdir(), "aieo-mac-"));
  extract("aarch64-apple-darwin", tmp, "arm64");
  extract("x86_64-apple-darwin", tmp, "x64");
  execFileSync("lipo", ["-create", join(tmp, "arm64"), join(tmp, "x64"), "-output", join(dir, "server", "aieo-macos")]);
  rmSync(tmp, { recursive: true, force: true });
} else {
  console.warn("lipo not found: the bundle's macOS binary is arm64 only (do not publish this bundle)");
  extract("aarch64-apple-darwin", join(dir, "server"), "aieo-macos");
}

const host = { linux: "aieo-linux", darwin: "aieo-macos", win32: "aieo.exe" }[process.platform];
const tools = await listTools(join(dir, "server", host));

cpSync(join(root, "plugins/aieo/assets/icon.png"), join(dir, "icon.png"));
cpSync(join(root, "LICENSE"), join(dir, "LICENSE"));

const description =
  "SEO and AI-search checks for a site's build output: sitemap, robots.txt, canonical, hreflang, links, llms.txt, structured data, framework and hosting config. Read-only.";
const manifest = {
  manifest_version: "0.3",
  name: "aieo",
  display_name: "aieo",
  version,
  description,
  long_description:
    "aieo reads what a site's build produced and what the framework and host declare, and reports defects with the build file and the source file to change. It also reads crawler access logs and Search Console / Bing data that the aieo CLI collected, against the build. Every tool is read-only and nothing leaves the machine: IndexNow submissions and aieo.ee uploads are returned as plans for a person to run with the CLI.",
  author: { name: "Web-Casa", url: "https://aieo.ee" },
  repository: { type: "git", url: `https://github.com/${REPO}` },
  homepage: `https://github.com/${REPO}`,
  documentation: "https://aieo.ee/docs/cli",
  icon: "icon.png",
  license: "MIT",
  keywords: ["seo", "geo", "ai-search", "sitemap", "llms.txt", "structured-data"],
  server: {
    type: "binary",
    entry_point: "server/aieo-linux",
    mcp_config: {
      command: "${__dirname}/server/aieo-linux",
      args: ["mcp", "-C", "${user_config.project}"],
      platform_overrides: {
        darwin: { command: "${__dirname}/server/aieo-macos" },
        win32: { command: "${__dirname}/server/aieo.exe" },
      },
    },
  },
  user_config: {
    project: {
      type: "directory",
      title: "Site project",
      description: "The website's project directory (where it is built and where aieo.toml lives). Tools can also name another directory with their `project` argument.",
      required: true,
    },
  },
  tools,
  tools_generated: false,
  compatibility: { platforms: ["darwin", "win32", "linux"] },
};
writeFileSync(join(dir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");

const file = `aieo-v${version}.mcpb`;
execFileSync("npx", ["-y", MCPB_CLI, "validate", join(dir, "manifest.json")], { stdio: "inherit" });
execFileSync("npx", ["-y", MCPB_CLI, "pack", dir, join(out, file)], { stdio: "inherit" });
const sha = createHash("sha256").update(readFileSync(join(out, file))).digest("hex");
writeFileSync(join(out, `${file}.sha256`), `${sha}  ${file}\n`);

const server = {
  $schema: "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json",
  name: REGISTRY_NAME,
  title: "aieo",
  description: "SEO and AI-search checks for a website's build output, fixed in the source files",
  repository: { url: `https://github.com/${REPO}`, source: "github" },
  websiteUrl: "https://aieo.ee/docs/cli",
  version,
  packages: [
    {
      registryType: "mcpb",
      identifier: `https://github.com/${REPO}/releases/download/v${version}/${file}`,
      fileSha256: sha,
      transport: { type: "stdio" },
    },
  ],
};
writeFileSync(join(out, "server.json"), JSON.stringify(server, null, 2) + "\n");
console.log(`wrote ${join(out, file)} (sha256 ${sha}) and server.json`);
