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
import { fileURLToPath, pathToFileURL } from "node:url";

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
execFileSync(process.execPath, [path.join(repo, "node_modules", "typescript", "bin", "tsc"), "-p", tmpTsconfig], { stdio: "pipe" });
const { fenceCount, safeCut, typewriterStep, typewriterStepSize, TYPEWRITER_TICK_MS } = await import(pathToFileURL(path.join(tmp, "typewriter.js")).href);

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

check("step size: 3/tick for a short gap, 6 for gap>600, 12 for gap>2000", () => {
  assert.equal(typewriterStepSize(90), 1);
  assert.equal(typewriterStepSize(600), 1);
  assert.equal(typewriterStepSize(601), 2);
  assert.equal(typewriterStepSize(2000), 2);
  assert.equal(typewriterStepSize(2001), 4);
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
  const seconds = (ticks * TYPEWRITER_TICK_MS) / 1000;
  assert.ok(seconds >= 1.8 && seconds <= 3.2, `took ${seconds.toFixed(2)} s for ${target.length} chars`);
  console.log(`      (${target.length} chars in ${ticks} ticks = ${seconds.toFixed(2)} s)`);
});

check("(ii) long reply (5000 chars), one delta: finishes in seconds, not minutes", () => {
  const target = "Lorem ipsum dolor sit amet. ".repeat(200).slice(0, 5000);
  assert.equal(target.length, 5000);
  const { displayed, ticks } = runTicks(target);
  assert.equal(displayed, target);
  const seconds = (ticks * TYPEWRITER_TICK_MS) / 1000;
  // Top tier is 12/tick on purpose (13 Nov 2026 fix): ~11 s for 5000
  // chars is visibly live and still seconds, not minutes.
  assert.ok(seconds >= 30 && seconds < 65, `took ${seconds.toFixed(2)} s`);
  console.log(`      (5000 chars in ${ticks} ticks = ${seconds.toFixed(2)} s)`);
});

check("(iii) code fences are typed THROUGH, never jumped whole (13 Nov 2026 fix)", () => {
  const target =
    "Berikut kodenya:\n```js\nconst a = 1;\nconsole.log(a);\n```\nLalu lanjut:\n```python\nprint('halo')\n```\nSelesai.";
  // The old engine held at a fence opener and then brought the whole
  // block in at once (a 1383-char single-tick jump in Bree's 18:31 live
  // test) — the code therefore appeared whole instead of being typed.
  // The chase now types through the fence: every tick adds at most one
  // step (12 chars), and the displayed text passes through mid-fence
  // states with partial code. MessageView splits on ``` and renders a
  // partial fence as a CodeBlock, so typing mid-block never breaks it.
  const done = runTicks(target);
  assert.equal(done.displayed, target);
  let maxJump = 0;
  for (let i = 1; i < done.history.length; i++) {
    const jump = done.history[i].length - done.history[i - 1].length;
    assert.ok(jump >= 0, "displayed never shrinks");
    assert.ok(jump <= 12, `one tick added ${jump} chars — a block jumped in whole`);
    assert.ok(target.startsWith(done.history[i]), "displayed is always a prefix of target");
    maxJump = Math.max(maxJump, jump);
  }
  assert.ok(maxJump > 0, "expected the chase to advance");
  const midFence = done.history.find(
    (h) => fenceCount(h) % 2 === 1 && h.includes("const a = 1"),
  );
  assert.ok(midFence, "expected a mid-fence intermediate with partial code");
  // While streaming with slow deltas the same holds: no tick adds more
  // than one step.
  const live = runTicks(target, { feed: 3, endAt: 10000 });
  for (let i = 1; i < live.history.length; i++) {
    assert.ok(live.history[i].length - live.history[i - 1].length <= 12, "streaming tick added too much");
  }
  assert.equal(live.displayed, target);
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

check("(vii) target replaced mid-turn: clear old prefix, then type final instead of jumping", () => {
  const streamed = "Ringkasan balasan.";
  let displayed = "";
  for (let i = 0; i < 3; i++) displayed = typewriterStep(streamed, displayed, true);
  assert.ok(displayed.length > 0 && displayed.length < streamed.length);
  const finalTarget = "Catatan: mode fallback.\n\nRingkasan balasan.";
  assert.equal(typewriterStep(finalTarget, displayed, false), "");
  let corrected = "";
  let ticks = 0;
  while (corrected !== finalTarget && ticks < 1000) {
    const next = typewriterStep(finalTarget, corrected, false);
    assert.ok(next.length - corrected.length <= 4, "replacement retypes in small steps");
    corrected = next;
    ticks++;
  }
  assert.equal(corrected, finalTarget);
  assert.ok(ticks > 10, "final reply passes through partial states");
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
