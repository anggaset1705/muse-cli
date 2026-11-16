// Node assertion tests for the live-typewriter-in-Ran-card round
// (13 Nov 2026, Bree: code must be typed live inside the "Ran ..."
// card, coloured like VS Code; raw tool-call JSON must never flash in
// the chat). Same pattern as test-ui-hermes.mjs / test-typewriter.mjs:
//   node scripts/test-ran-live.mjs
// It transpiles the REAL src/lib files with the repo's tsc into a temp
// dir and drives them directly, so the assertions exercise the shipped
// code, not a copy of it. Exits non-zero on any failure.
//
// Fixture: scripts/fixtures/calculator-toolcall.txt — the EXACT tool-call
// fence the model emitted for Bree's 17:37 turn ("buatkan scrip python
// kalkulator"), reconstructed from the real file the agent wrote
// (kalkulator_neumorphism.py, 5680 chars — the same count the tool
// result reported) + the exact {"tool_calls": [...]} schema the gateway
// parser accepts (see muse-agent tests/integration.rs). gateway.log
// carries no turn content (only start/store lines), so the log cannot
// supply the bytes; this fixture is the closest ground truth.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "ran-live-test-"));
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
    files: [
      path.join(repo, "src", "lib", "highlight.ts"),
      path.join(repo, "src", "lib", "typewriter.ts"),
      path.join(repo, "src", "lib", "replyEnvelope.ts"),
    ],
  }),
);
execFileSync(process.execPath, [path.join(repo, "node_modules", "typescript", "bin", "tsc"), "-p", tmpTsconfig], { stdio: "pipe" });
const { highlightCode, langFromPath } = await import(pathToFileURL(path.join(tmp, "highlight.js")).href);
const { liveContentStep, typewriterStepSize, TYPEWRITER_TICK_MS } = await import(pathToFileURL(path.join(tmp, "typewriter.js")).href);
const { stripToolCallBlocks, streamingReplyDisplay, unwrapReplyEnvelope } = await import(pathToFileURL(path.join(tmp, "replyEnvelope.js")).href);

const fixture = fs.readFileSync(
  path.join(repo, "scripts", "fixtures", "calculator-toolcall.txt"),
  "utf8",
);
const realCode = JSON.parse(
  fixture.replace(/^```json\n/, "").replace(/\n```$/, ""),
).tool_calls[0].arguments.content;

let passed = 0;
function check(name, fn) {
  fn();
  passed++;
  console.log(`  PASS ${name}`);
}

// --- liveContentStep: plain prefix chase, no fence rules -------------
check("liveContentStep: types the real 5680-char file progressively, never at once", () => {
  let cur = "";
  let ticks = 0;
  while (cur.length < realCode.length && ticks < 5000) {
    const next = liveContentStep(realCode, cur);
    assert.ok(next.length > cur.length, "each tick must advance");
    assert.ok(next.length <= cur.length + 12, "one tick adds at most 12 chars");
    assert.ok(realCode.startsWith(next), "displayed must stay a prefix of the target");
    cur = next;
    ticks++;
  }
  assert.equal(cur, realCode);
  // Visibly live: ~8 s of typing at 16 ms ticks, not a single jump.
  assert.ok(ticks > 50, `expected >50 ticks, got ${ticks}`);
  assert.ok(ticks < 3000, `expected <1000 ticks, got ${ticks}`);
});

check("liveContentStep: small target takes several ticks, not one jump", () => {
  const target = "print('hello world')";
  let cur = "";
  let ticks = 0;
  while (cur.length < target.length && ticks < 100) {
    cur = liveContentStep(target, cur);
    ticks++;
  }
  assert.equal(cur, target);
  assert.ok(ticks > 1, `expected >1 tick, got ${ticks}`);
});

check("liveContentStep: non-prefix current jumps to the target", () => {
  assert.equal(liveContentStep("abcdef", "xyz"), "abcdef");
  assert.equal(liveContentStep("abcdef", "abcdef"), "abcdef");
});

check("liveContentStep: backticks do not stall it (raw code, no fence rules)", () => {
  const target = "const s = `a${b}c`;\nconst t = `x`;";
  let cur = "";
  let ticks = 0;
  while (cur.length < target.length && ticks < 200) {
    cur = liveContentStep(target, cur);
    ticks++;
  }
  assert.equal(cur, target);
});

// --- langFromPath ------------------------------------------------------
check("langFromPath: extension map", () => {
  assert.equal(langFromPath("kalkulator_neumorphism.py"), "python");
  assert.equal(langFromPath("src\\deep\\app.py"), "python");
  assert.equal(langFromPath("a.js"), "javascript");
  assert.equal(langFromPath("a.jsx"), "javascript");
  assert.equal(langFromPath("a.ts"), "typescript");
  assert.equal(langFromPath("a.tsx"), "typescript");
  assert.equal(langFromPath("a.rs"), "rust");
  assert.equal(langFromPath("a.sh"), "bash");
  assert.equal(langFromPath("a.json"), "json");
  assert.equal(langFromPath("a.sql"), "sql");
  assert.equal(langFromPath("a.html"), "html");
  assert.equal(langFromPath("a.css"), "css");
  assert.equal(langFromPath("a.yaml"), "yaml");
  assert.equal(langFromPath("a.yml"), "yaml");
  assert.equal(langFromPath("a.txt"), "");
  assert.equal(langFromPath("Dockerfile"), "");
  assert.equal(langFromPath("noext"), "");
  assert.equal(langFromPath(undefined), "");
});

// --- stripToolCallBlocks: the real Bree case ---------------------------
check("stripToolCallBlocks: the exact calculator tool-call fence is removed", () => {
  assert.equal(stripToolCallBlocks(fixture), "");
});

check("stripToolCallBlocks: fence + summary leaves only the summary", () => {
  const text = `${fixture}\nSelesai. Script kalkulator sudah dibuat di kalkulator_neumorphism.py (5680 karakter).`;
  const out = stripToolCallBlocks(text);
  assert.ok(!out.includes("tool_calls"), "no tool_calls left");
  assert.ok(!out.includes("```json"), "no json fence left");
  assert.ok(!out.includes("import tkinter"), "no raw code left");
  assert.ok(out.includes("Selesai. Script kalkulator sudah dibuat"), "summary intact");
});

check("stripToolCallBlocks: unclosed tool-call fence is hidden from its opener", () => {
  const partial = fixture.slice(0, 400); // cut mid-content, fence never closes
  assert.equal(stripToolCallBlocks(partial), "");
  const withNarration = `Saya buatkan dulu ya.\n${partial}`;
  assert.equal(stripToolCallBlocks(withNarration), "Saya buatkan dulu ya.\n");
});

check("stripToolCallBlocks: legit JSON and code fences are kept", () => {
  const legit = '```json\n{"name": "kalkulator", "version": 1}\n```';
  assert.equal(stripToolCallBlocks(legit), legit);
  const code = "```python\nprint('halo')\n```";
  assert.equal(stripToolCallBlocks(code), code);
  const plain = "Halo, kabar baik. Ada yang bisa saya bantu?";
  assert.equal(stripToolCallBlocks(plain), plain);
});

check("stripToolCallBlocks: bare whole-text tool-call object is removed", () => {
  assert.equal(stripToolCallBlocks('{"tool_calls":[{"name":"read_file","arguments":{"path":"x"}}]}'), "");
  assert.equal(stripToolCallBlocks('{"actions":[{"name":"read_file"}]}'), "");
  // ...but a bare status/summary object is prose for the envelope logic.
  const env = '{"status":"done","summary":"Halo!"}';
  assert.equal(stripToolCallBlocks(env), env);
});

check("stripToolCallBlocks: bare tool-call JSON still streaming is hidden", () => {
  assert.equal(stripToolCallBlocks('{"tool_calls":[{"name":"write_file","arguments":{"path":"x","content":"im'), "");
});

// --- streaming display / finish unwrap ---------------------------------
check("streamingReplyDisplay: tool-call fence streaming never renders", () => {
  assert.equal(streamingReplyDisplay(fixture.slice(0, 400)), "");
  assert.equal(streamingReplyDisplay(fixture), "");
  const done = `${fixture}\nSelesai. Script kalkulator sudah dibuat.`;
  assert.equal(streamingReplyDisplay(done), "Selesai. Script kalkulator sudah dibuat.");
});

check("streamingReplyDisplay: status/summary envelope still unwraps", () => {
  assert.equal(streamingReplyDisplay('json {"status": "done", "summary": "Halo!"}'), "Halo!");
  assert.equal(streamingReplyDisplay("Halo, kabar baik."), "Halo, kabar baik.");
});

check("unwrapReplyEnvelope: tool-call fence + summary saves only the summary", () => {
  const text = `${fixture}\nSelesai. Script kalkulator sudah dibuat.`;
  assert.equal(unwrapReplyEnvelope(text), "Selesai. Script kalkulator sudah dibuat.");
  assert.equal(unwrapReplyEnvelope(fixture), "");
});

check("unwrapReplyEnvelope: status/summary envelope still unwraps, legit JSON kept", () => {
  assert.equal(
    unwrapReplyEnvelope('```json\n{"status": "done", "summary": "Selesai."}\n```'),
    "Selesai.",
  );
  const legit = '```json\n{"name": "kalkulator", "version": 1}\n```';
  assert.equal(unwrapReplyEnvelope(legit), legit);
});

// --- highlight while typing: growing prefixes stay coloured + safe -----
check("highlightCode: growing prefixes of the real file stay highlighted and escaped", () => {
  for (const n of [100, 500, 2000, realCode.length]) {
    const prefix = realCode.slice(0, n);
    const html = highlightCode(prefix, "python");
    assert.ok(html.includes("tok-kw"), `prefix ${n} must contain keyword spans`);
    const withoutSpans = html.replace(/<\/?span[^>]*>/g, "");
    assert.ok(!withoutSpans.includes("<"), `prefix ${n} must not leak raw "<"`);
  }
  const full = highlightCode(realCode, "python");
  assert.ok(full.includes("tok-str"), "strings coloured");
  // The real file has no `#` comments (its `#` chars are all inside
  // string literals like '#e0e5ec'), so comments are proven on a
  // synthetic snippet instead.
  assert.ok(highlightCode("# hitung total\ndef f():\n    return 1", "python").includes("tok-com"), "comments coloured");
});

console.log(`\n${passed} passed, 0 failed`);
