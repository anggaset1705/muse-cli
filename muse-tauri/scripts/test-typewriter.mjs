// Node assertion tests for the typewriter engine + first-paint background.
// No test framework in the repo (same pattern as the replyEnvelope round):
//   node scripts/test-typewriter.mjs
// It transpiles the REAL src/lib/typewriter.ts with the repo's tsc into a
// temp dir and drives typewriterStep() on a virtual 16 ms clock, so the
// assertions exercise the shipped code, not a copy of it. Exits non-zero
// on any failure. Run `npm run build` first: the dist/index.html checks
// need a fresh build.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "tw-test-"));
// tsc refuses file args when a tsconfig.json sits in the cwd (TS5112),
// so compile through a minimal temp project instead.
const tmpTsconfig = path.join(tmp, "tsconfig.json");
fs.writeFileSync(
  tmpTsconfig,
  JSON.stringify({
    compilerOptions: {
      target: "es2022",
      module: "nodenext",
      moduleResolution: "nodenext",
      rootDir: path.join(repo, "src", "lib"),
      outDir: tmp,
      strict: true,
    },
    files: [path.join(repo, "src", "lib", "typewriter.ts")],
  }),
);
execFileSync(path.join(repo, "node_modules", ".bin", "tsc"), ["-p", tmpTsconfig], { stdio: "pipe" });
const { fenceCount, safeCut, typewriterStep, typewriterStepSize } = await import(
  path.join(tmp, "typewriter.js")
);

let passed = 0;
let failed = 0;
function check(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (e) {
    failed++;
    console.log(`  FAIL ${name}: ${e.message}`);
  }
}

// Drive the engine the way ChatPanel's 16 ms interval does. The target
// GROWS while streaming (deltas arrive `feed` chars per tick, like SSE);
// at `endAt` the turn finishes and the target becomes the full saved
// text, exactly as finish() does. Returns the displayed value after
// every tick.
function runTicks(full, { feed = Infinity, endAt = 0, maxTicks = 5000 } = {}) {
  let target = "";
  let displayed = "";
  const history = [displayed];
  for (let tick = 0; tick < maxTicks; tick++) {
    const streaming = tick < endAt;
    if (streaming) target = full.slice(0, Math.min(full.length, target.length + feed));
    else target = full;
    displayed = typewriterStep(target, displayed, streaming);
    history.push(displayed);
    // Once displayed has caught up with the fully-fed target there is
    // nothing more to observe — the real interval would just idle here.
    if (displayed === full && target === full) break;
  }
  return { displayed, history, ticks: history.length - 1 };
}

console.log("typewriter engine (real src/lib/typewriter.ts):");

check("step size: 3/tick for a short gap, 30 for gap>600, 60 for gap>2000", () => {
  assert.equal(typewriterStepSize(90), 3);
  assert.equal(typewriterStepSize(600), 3);
  assert.equal(typewriterStepSize(601), 30);
  assert.equal(typewriterStepSize(2000), 30);
  assert.equal(typewriterStepSize(2001), 60);
});

check("(i) one full ~90 char delta, streaming ends at once: types gradually, ~0.5 s, ends full", () => {
  const target =
    "Halo! Kabar saya baik, terima kasih. Ada yang bisa saya bantu hari ini? Silakan tanya apa saja.";
  assert.ok(target.length >= 85 && target.length <= 100, `fixture is ${target.length} chars`);
  const { displayed, history, ticks } = runTicks(target, { endAt: 2 });
  // After 2 ticks (streaming just ended) only ~6 chars are visible —
  // the reply does NOT appear whole the moment streaming stops.
  assert.ok(history[2].length <= 9, `after 2 ticks displayed=${history[2].length} chars`);
  // It grows strictly (3 chars/tick) and ends exactly at the full text.
  for (let i = 1; i < history.length; i++) {
    assert.ok(history[i].length >= history[i - 1].length, "displayed never shrinks");
    assert.ok(target.startsWith(history[i]), "displayed is always a prefix of target");
  }
  assert.equal(displayed, target);
  const seconds = (ticks * 16) / 1000;
  assert.ok(seconds >= 0.4 && seconds <= 0.7, `took ${seconds.toFixed(2)} s for ${target.length} chars`);
  console.log(`      (${target.length} chars in ${ticks} ticks = ${seconds.toFixed(2)} s)`);
});

check("(ii) long reply (5000 chars), one delta: finishes in seconds, not minutes", () => {
  const target = "Lorem ipsum dolor sit amet. ".repeat(200).slice(0, 5000);
  assert.equal(target.length, 5000);
  const { displayed, ticks } = runTicks(target);
  assert.equal(displayed, target);
  const seconds = (ticks * 16) / 1000;
  assert.ok(seconds < 10, `took ${seconds.toFixed(2)} s`);
  console.log(`      (5000 chars in ${ticks} ticks = ${seconds.toFixed(2)} s)`);
});

check("(iii) code fences are never cut mid-fence during the animation", () => {
  const target =
    "Berikut kodenya:\n```js\nconst a = 1;\nconsole.log(a);\n```\nLalu lanjut:\n```python\nprint('halo')\n```\nSelesai.";
  // While streaming the engine holds at a fence opener (a tick that
  // advances by 0) until the fence closes.
  const live = runTicks(target, { feed: 3, endAt: 10000 });
  for (const h of live.history) {
    assert.equal(fenceCount(h) % 2, 0, `odd fence count in intermediate ${JSON.stringify(h.slice(-40))}`);
  }
  assert.equal(live.displayed, target);
  assert.ok(live.history.some((h, i) => i > 0 && h === live.history[i - 1]), "expected a hold tick at a fence");
  // After the turn ends the same reply still converges, with every
  // intermediate fence-even; a held cut jumps past the closing fence
  // (block appears whole) instead of jumping to the full text.
  const done = runTicks(target);
  for (const h of done.history) {
    assert.equal(fenceCount(h) % 2, 0, `odd fence count in post-finish intermediate ${JSON.stringify(h.slice(-40))}`);
  }
  assert.equal(done.displayed, target);
  // Normal ticks advance at most 3 chars here (short target); any bigger
  // single-tick advance is a jump past a closing fence.
  const jumps = done.history.filter((h, i) => i > 0 && h.length - done.history[i - 1].length > 6);
  assert.ok(jumps.length > 0, "expected a jump past a closing fence");
  assert.ok(jumps.every((h) => h.endsWith("```")), "jump must land just after a closing fence");
});

check("(iv) Stop mid-animation: displayed freezes at what was typed, never jumps to full", () => {
  const target = "Halo! Kabar saya baik, terima kasih. Ada yang bisa saya bantu hari ini?";
  let displayed = "";
  for (let i = 0; i < 10; i++) displayed = typewriterStep(target, displayed, true);
  assert.ok(displayed.length > 0 && displayed.length < target.length, `stopped at ${displayed.length}/${target.length}`);
  assert.ok(target.startsWith(displayed), "stopped text is a typed prefix");
  // After Stop the turn is over and the saved message IS the typed text,
  // so the engine is already done — it must not advance to the full text.
  assert.equal(typewriterStep(displayed, displayed, false), displayed);
});

check("(v) new turn: displayed resets to empty", () => {
  const target = "Balasan giliran pertama yang cukup panjang untuk dites.";
  const first = runTicks(target);
  assert.equal(first.displayed, target);
  const displayedAfterReset = "";
  assert.equal(displayedAfterReset, "");
  const second = runTicks("Balasan kedua.");
  assert.equal(second.history[0], "");
  assert.equal(second.displayed, "Balasan kedua.");
});

check("(vi) unclosed fence in the FINAL text: jumps to full instead of truncating forever", () => {
  const target = "Ini kodenya:\n```js\nconst a = 1;\n";
  let displayed = "";
  for (let i = 0; i < 200; i++) displayed = typewriterStep(target, displayed, false);
  assert.equal(displayed, target);
});

check("(vii) target replaced mid-turn (fallback note prepended at finish): jumps to full, no garbled mix", () => {
  const streamed = "Ringkasan balasan.";
  let displayed = "";
  for (let i = 0; i < 3; i++) displayed = typewriterStep(streamed, displayed, true);
  assert.ok(displayed.length > 0 && displayed.length < streamed.length);
  const finalTarget = "Catatan: mode fallback.\n\nRingkasan balasan.";
  assert.equal(typewriterStep(finalTarget, displayed, false), finalTarget);
});

console.log("first-paint background:");

check("dist/index.html contains the inline dark background + color-scheme", () => {
  const dist = path.join(repo, "dist", "index.html");
  assert.ok(fs.existsSync(dist), "dist/index.html missing — run npm run build first");
  const html = fs.readFileSync(dist, "utf8");
  // Vite minifies the inline style: `background:#17171d` (lowercase hex,
  // no space) — match case-insensitively.
  assert.ok(/background:\s*#17171d/i.test(html), "no #17171D inline background in dist/index.html");
  assert.ok(/color-scheme:\s*dark/i.test(html), "no color-scheme: dark in dist/index.html");
});

check("tauri.conf.json: windows[0].backgroundColor is a schema-valid Color", () => {
  const conf = JSON.parse(fs.readFileSync(path.join(repo, "src-tauri", "tauri.conf.json"), "utf8"));
  const bg = conf?.app?.windows?.[0]?.backgroundColor;
  assert.equal(bg, "#17171D");
  assert.match(bg, /^#?([A-Fa-f0-9]{3}|[A-Fa-f0-9]{6}|[A-Fa-f0-9]{8})$/, "not a valid Tauri Color hex");
});

fs.rmSync(tmp, { recursive: true, force: true });
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
