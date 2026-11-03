// Build the Rust gateway sidecar and place it next to the Tauri app exe.
//
// Repo layout (fresh clone of anggaset1705/muse-cli):
//   muse-cli/
//     muse-tauri/    <- this app (run this script from here)
//     muse-gateway/  <- Rust workspace (cargo workspace, binary muse-gateway)
//
// Steps:
//   1. cargo build --release -p muse-gateway   (in muse-gateway/)
//   2. copy muse-gateway[.exe] to muse-tauri/src-tauri/target/release/
//      (the packaged-release location the app looks for first)
//   3. copy it again to muse-tauri/src-tauri/binaries/
//      muse-gateway-<target-triple>[.exe]  (Tauri externalBin sidecar —
//      bundled next to the app exe by `npm run tauri build`, so a user
//      installing from the MSI/NSIS/.deb gets a working backend)
//
// Target triple comes from `rustc -vV` (host), with a platform fallback.
//
// Run:  npm run build:gateway        (from muse-tauri/)
//   or: node scripts/build-gateway.mjs
// Env override (old VM layout): MUSE_GATEWAY_DIR=/path/to/muse-rust-backend

import { spawnSync, execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const museTauriDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = dirname(museTauriDir);
const gatewayDir =
  process.env.MUSE_GATEWAY_DIR || join(repoRoot, "muse-gateway");
const isWindows = process.platform === "win32";
const binName = isWindows ? "muse-gateway.exe" : "muse-gateway";
const destDir = join(museTauriDir, "src-tauri", "target", "release");

function run(cmd, args, cwd) {
  console.log(`$ ${cmd} ${args.join(" ")}  (cwd: ${cwd})`);
  const r = spawnSync(cmd, args, { cwd, stdio: "inherit", shell: isWindows });
  if (r.error) throw new Error(`${cmd} failed to start: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`${cmd} exited with ${r.status}`);
}

const manifest = join(gatewayDir, "Cargo.toml");
if (!existsSync(manifest)) {
  throw new Error(
    `muse-gateway workspace not found at ${gatewayDir} ` +
      `(expected ${manifest}). Clone the repo with the muse-gateway/ ` +
      `folder next to muse-tauri/, or set MUSE_GATEWAY_DIR.`,
  );
}

run("cargo", ["build", "--release", "-p", "muse-gateway", "--manifest-path", manifest], gatewayDir);

const src = join(gatewayDir, "target", "release", binName);
if (!existsSync(src)) throw new Error(`gateway binary not found after build: ${src}`);
mkdirSync(destDir, { recursive: true });
const dest = join(destDir, binName);
copyFileSync(src, dest);
console.log(`gateway sidecar copied: ${src} -> ${dest}`);

// Sidecar copy for the Tauri bundler (externalBin): the file must be named
// muse-gateway-<target-triple>[.exe] under src-tauri/binaries/.
function targetTriple() {
  try {
    const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
    const m = out.match(/^host:\s*(\S+)/m);
    if (m) return m[1];
  } catch {
    // fall through to the platform map below
  }
  if (isWindows) return "x86_64-pc-windows-msvc";
  if (process.platform === "darwin")
    return process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  return "x86_64-unknown-linux-gnu";
}
const triple = targetTriple();
const binariesDir = join(museTauriDir, "src-tauri", "binaries");
mkdirSync(binariesDir, { recursive: true });
const sidecarName = `muse-gateway-${triple}${isWindows ? ".exe" : ""}`;
const sidecarDest = join(binariesDir, sidecarName);
copyFileSync(src, sidecarDest);
console.log(`gateway sidecar (bundle) copied: ${src} -> ${sidecarDest}`);
console.log(`Next: npm run tauri build   (builds the app; the sidecar is found next to the exe)`);
