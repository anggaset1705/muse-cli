// Regression tests for the 13 Nov 2026 fix-live round (Bree's 18:31 live
// test FAILED: the reply appeared all at once + the chat code block was
// not coloured). Fixture scripts/fixtures/msg-1831.txt is the EXACT
// stored assistant message from that turn (read-only leveldb probe of
// Bree's PC), not a made-up sample. Run:
//   node scripts/test-fix-live.mjs
// It transpiles the REAL src/lib/typewriter.ts + src/lib/highlight.ts
// with the repo's tsc into a temp dir and drives them in Node. Exits
// non-zero on any failure.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fixlive-test-"));
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
      path.join(repo, "src", "lib", "typewriter.ts"),
      path.join(repo, "src", "lib", "highlight.ts"),
    ],
  }),
);
execFileSync(process.execPath, [path.join(repo, "node_modules", "typescript", "bin", "tsc"), "-p", tmpTsconfig], { stdio: "pipe" });
const { typewriterStep, TYPEWRITER_TICK_MS } = await import(pathToFileURL(path.join(tmp, "typewriter.js")).href);
const { highlightCode, detectLang } = await import(pathToFileURL(path.join(tmp, "highlight.js")).href);

const msg = fs.readFileSync(path.join(repo, "scripts", "fixtures", "msg-1831.txt"), "utf8");

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

// Drive the engine the way ChatPanel's 16 ms interval does, with the
// whole reply arriving in a single delta (the local-tools path sends one
// full delta after stabilisation — exactly what happened at 18:31).
function runTicks(full) {
  let displayed = "";
  const history = [displayed];
  while (displayed !== full && history.length < 5000) {
    displayed = typewriterStep(full, displayed, false);
    history.push(displayed);
  }
  return { displayed, history, ticks: history.length - 1 };
}

// The MessageView fence split, mirrored exactly (src/components/
// MessageView.tsx): parts are [text, lang, code, text, lang, code, ...].
function splitFences(content) {
  return content.split(/```(\w*)\n?/);
}

console.log("fix-live regression (real 18:31 stored message):");

check("18:31 reply types progressively: several visible seconds, no whole-block jump", () => {
  assert.equal(msg.length, 1584, `fixture must be the exact 1584-char message, got ${msg.length}`);
  const { displayed, history, ticks } = runTicks(msg);
  assert.equal(displayed, msg);
  const seconds = (ticks * TYPEWRITER_TICK_MS) / 1000;
  assert.ok(seconds >= 15, `typed in ${seconds.toFixed(2)} s — too fast to see (old engine: 0.40 s)`);
  assert.ok(seconds <= 40, `typed in ${seconds.toFixed(2)} s — unreasonably slow`);
  let maxJump = 0;
  for (let i = 1; i < history.length; i++) {
    const jump = history[i].length - history[i - 1].length;
    assert.ok(jump >= 0, "displayed never shrinks");
    assert.ok(jump <= 12, `tick ${i} added ${jump} chars — a block jumped in whole`);
    maxJump = Math.max(maxJump, jump);
  }
  // The code block must grow progressively: some intermediate is
  // mid-fence (odd ``` count) with partial code already visible.
  const mid = history.find((h) => (h.match(/```/g) ?? []).length % 2 === 1 && h.includes("def tambah"));
  assert.ok(mid, "expected a mid-fence intermediate with partial code");
  assert.ok(!mid.includes("if __name__"), "mid-fence state must not already contain the end of the block");
  console.log(`      (${msg.length} chars in ${ticks} ticks = ${seconds.toFixed(2)} s, max jump ${maxJump})`);
});

check("18:31 bare fence: language is detected as python and the code is coloured", () => {
  const parts = splitFences(msg);
  assert.equal(parts[1], "", "the 18:31 fence has no language tag — that is the bug");
  const code = parts[2];
  assert.ok(code.length > 1000, `code part is ${code.length} chars`);
  // Old behaviour (the bug): an empty language falls back to plain text.
  assert.ok(!/<span/.test(highlightCode(code, "")), "empty language must stay plain (documents the bug)");
  // Fixed behaviour: detection from the content.
  assert.equal(detectLang(code), "python");
  const html = highlightCode(code, detectLang(code));
  assert.ok(/tok-kw/.test(html), "keywords must be coloured");
  assert.ok((html.match(/tok-kw/g) ?? []).length >= 10, "expected many keywords");
  assert.ok(/tok-str/.test(html), "strings must be coloured");
  assert.ok(/tok-com/.test(html), "the # comment must be coloured");
  assert.ok(html.includes("def"), "code text must survive highlighting");
});

check("partial fences (mid-typing) still render as a code block, not broken markdown", () => {
  // Simulate the typewriter at several points inside the code block.
  const cutPoints = [300, 600, 900, 1200, 1500];
  for (const n of cutPoints) {
    const prefix = msg.slice(0, n);
    const parts = splitFences(prefix);
    // A prefix ending inside the fence splits into [text, lang, code].
    assert.ok(parts.length >= 3, `prefix ${n}: split gave ${parts.length} parts`);
    const code = parts[2] ?? "";
    assert.ok(code.length > 0, `prefix ${n}: code part must not be empty`);
    const html = highlightCode(code, detectLang(code));
    assert.ok(typeof html === "string" && html.length > 0, `prefix ${n}: highlight must return text`);
  }
});

check("detectLang: content heuristics per language", () => {
  assert.equal(detectLang("def f(x):\n    return x + 1\n"), "python");
  assert.equal(detectLang("import tkinter as tk\nprint(\"hi\")\n"), "python");
  assert.equal(detectLang("const a = 1;\nconsole.log(a);\n"), "javascript");
  assert.equal(detectLang("function f() {\n  return 1;\n}\n"), "javascript");
  assert.equal(detectLang("fn main() {\n    println!(\"hi\");\n}\n"), "rust");
  assert.equal(detectLang("let mut x = 1;\n"), "rust");
  assert.equal(detectLang("#!/bin/bash\necho \"hi\"\n"), "bash");
  assert.equal(detectLang('{"a": 1, "b": [1, 2]}'), "json");
  assert.equal(detectLang("SELECT id, name FROM users WHERE id = 1;"), "sql");
  assert.equal(detectLang("<div class=\"x\">hello</div>\n"), "html");
  assert.equal(detectLang(".btn {\n  color: red;\n  margin: 0;\n}\n"), "css");
  assert.equal(detectLang("name: foo\nversion: 1.2\n"), "yaml");
  assert.equal(detectLang("hello world, just prose\n"), "");
  assert.equal(detectLang(""), "");
});

check("pacing: a ~1000-char reply takes seconds, a short reply stays ~0.5 s", () => {
  const long = "Lorem ipsum dolor sit amet. ".repeat(36).slice(0, 1000);
  assert.equal(long.length, 1000);
  const r1 = runTicks(long);
  const s1 = (r1.ticks * TYPEWRITER_TICK_MS) / 1000;
  assert.ok(s1 >= 10, `1000 chars typed in ${s1.toFixed(2)} s — too fast to see`);
  assert.ok(s1 <= 28, `1000 chars typed in ${s1.toFixed(2)} s — too slow`);
  const short = "Halo! Kabar saya baik, terima kasih. Ada yang bisa saya bantu hari ini? Silakan tanya apa saja.";
  const r2 = runTicks(short);
  const s2 = (r2.ticks * TYPEWRITER_TICK_MS) / 1000;
  assert.ok(s2 >= 1.8 && s2 <= 3.2, `~90 chars took ${s2.toFixed(2)} s`);
  console.log(`      (1000 chars: ${s1.toFixed(2)} s; ~90 chars: ${s2.toFixed(2)} s)`);
});

fs.rmSync(tmp, { recursive: true, force: true });
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
