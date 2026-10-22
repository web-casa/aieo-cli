// Assemble the npm packages from release archives:
//   node packaging/npm/build.mjs <version> <dir with aieo-v<version>-<target>.tar.gz|.zip> <out dir>
// Writes <out>/cli (the launcher) and <out>/cli-<platform>-<arch> (one per binary),
// each ready for `npm publish --access public`.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const [version, assets, out] = process.argv.slice(2);
if (!version || !assets || !out) {
  console.error("usage: node build.mjs <version> <assets dir> <out dir>");
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const TARGETS = {
  "x86_64-unknown-linux-musl": ["linux", "x64"],
  "aarch64-unknown-linux-musl": ["linux", "arm64"],
  "aarch64-apple-darwin": ["darwin", "arm64"],
  "x86_64-apple-darwin": ["darwin", "x64"],
  "x86_64-pc-windows-msvc": ["win32", "x64"],
};
const base = JSON.parse(readFileSync(join(here, "cli/package.json"), "utf8"));
const optional = {};
for (const [target, [os, cpu]] of Object.entries(TARGETS)) {
  const name = `@aieo/cli-${os}-${cpu}`;
  const dir = join(out, `cli-${os}-${cpu}`);
  mkdirSync(join(dir, "bin"), { recursive: true });
  const archive = join(assets, `aieo-v${version}-${target}.${os === "win32" ? "zip" : "tar.gz"}`);
  if (!existsSync(archive)) throw new Error(`missing ${archive}`);
  if (os === "win32") execFileSync("unzip", ["-o", "-j", archive, "aieo.exe", "-d", join(dir, "bin")]);
  else execFileSync("tar", ["-xzf", archive, "-C", join(dir, "bin"), "aieo"]);
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify(
      {
        name,
        version,
        description: `aieo binary for ${os}-${cpu}`,
        license: "MIT",
        repository: base.repository,
        os: [os],
        cpu: [cpu],
        files: ["bin"],
        preferUnplugged: true,
      },
      null,
      2
    ) + "\n"
  );
  optional[name] = version;
}
const cli = join(out, "cli");
mkdirSync(cli, { recursive: true });
cpSync(join(here, "cli/bin"), join(cli, "bin"), { recursive: true });
cpSync(join(here, "cli/README.md"), join(cli, "README.md"));
writeFileSync(join(cli, "package.json"), JSON.stringify({ ...base, version, optionalDependencies: optional }, null, 2) + "\n");
console.log(`npm packages for ${version} in ${out}`);
