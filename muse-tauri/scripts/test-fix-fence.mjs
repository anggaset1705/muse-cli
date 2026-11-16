// Regression tests for the 13 Nov 2026 fix-fence round (Bree's 20:53 live
// test FAILED: the reply showed the word "json" + an empty code block).
// Fixture scripts/fixtures/msg-2053.txt is the EXACT stored assistant
// message from that turn (read-only leveldb probe of Bree's PC), not a
// made-up sample. The mechanism, proven by replay: the frontend strip
// dropped the tool-call fence but left its "json" label line + the empty
// bare-fence shell behind. Run:
//   node scripts/test-fix-fence.mjs
// It transpiles the REAL src/lib/replyEnvelope.ts + src/lib/highlight.ts
// with the repo's tsc into a temp dir and drives them in Node. Exits
// non-zero on any failure.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fixfence-test-"));
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
      path.join(repo, "src", "lib", "replyEnvelope.ts"),
      path.join(repo, "src", "lib", "highlight.ts"),
    ],
  }),
);
execFileSync(process.execPath, [path.join(repo, "node_modules", "typescript", "bin", "tsc"), "-p", tmpTsconfig], { stdio: "pipe" });
const { stripToolCallBlocks, unwrapReplyEnvelope, streamingReplyDisplay } = await import(pathToFileURL(path.join(tmp, "replyEnvelope.js")).href);
const { highlightCode } = await import(pathToFileURL(path.join(tmp, "highlight.js")).href);

const msg2053 = fs.readFileSync(path.join(repo, "scripts", "fixtures", "msg-2053.txt"), "utf8");
const toolFence = fs.readFileSync(path.join(repo, "scripts", "fixtures", "calculator-toolcall.txt"), "utf8").trim();

let passed = 0;
let failed = 0;
function check(name, fn) {
  try {
    fn();
    passed++;
    console.log("  PASS " + name);
  } catch (e) {
    failed++;
    console.log("  FAIL " + name + ": " + (e && e.message ? e.message : e));
  }
}

// MessageView's fence split, mirrored here (no React in Node).
function splitParts(content) {
  return content.split(/```(\w*)\n?/);
}

check("stored 20:53 message is exactly the 17-char artifact", () => {
  assert.equal(msg2053, "json\n\n\n\n```\n\n\n```");
  assert.equal(msg2053.length, 17);
});

check("stored artifact renders as 'json' text + empty CodeBlock (the bug)", () => {
  // This documents the pre-fix render shape: split gives a "json" text
  // part and a bare fence with a whitespace-only code part.
  const parts = splitParts(msg2053);
  assert.equal(parts[0], "json\n\n\n\n");
  assert.equal(parts[1], "");
  assert.equal(parts[2].trim(), "");
});

check("strip removes the stored artifact entirely (live + saved)", () => {
  assert.equal(stripToolCallBlocks(msg2053), "");
  assert.equal(unwrapReplyEnvelope(msg2053), "");
  assert.equal(streamingReplyDisplay(msg2053), "");
});

check("label + real tool-call fence + empty shell replays to empty", () => {
  const raw = "json\n\n\n\n" + toolFence + "\n```\n\n\n```";
  assert.equal(stripToolCallBlocks(raw), "");
  assert.equal(unwrapReplyEnvelope(raw), "");
  assert.equal(streamingReplyDisplay(raw), "");
});

check("empty bare fence alone is removed", () => {
  assert.equal(stripToolCallBlocks("```\n\n\n```"), "");
  assert.equal(stripToolCallBlocks("Some prose\n```\n\n```\nMore prose"), "Some prose\nMore prose");
});

check("empty tagged fence alone is removed", () => {
  assert.equal(stripToolCallBlocks("```python\n\n```"), "");
  assert.equal(stripToolCallBlocks("```json\n```"), "");
});

check("label line alone (fence already gone upstream) is removed", () => {
  assert.equal(stripToolCallBlocks("json\n"), "");
  assert.equal(stripToolCallBlocks("json\n\n\n\n"), "");
  assert.equal(unwrapReplyEnvelope("json\n"), "");
});

check("label + tool-call fence (no trailing shell) leaves nothing", () => {
  assert.equal(stripToolCallBlocks("json\n\n\n\n" + toolFence), "");
});

check("tool-call fence without a label still leaves nothing", () => {
  assert.equal(stripToolCallBlocks(toolFence), "");
  assert.equal(unwrapReplyEnvelope(toolFence), "");
});

check("prose before the label is kept", () => {
  const raw = "Here is the plan:\njson\n\n\n\n" + toolFence + "\n```\n\n\n```";
  assert.equal(stripToolCallBlocks(raw), "Here is the plan:");
});

check("legit python fence with code is kept whole + coloured", () => {
  const code = "def tambah(a, b):\n    return a + b\n";
  const text = "Cara menjalankannya:\n```python\n" + code + "```";
  assert.equal(stripToolCallBlocks(text), text);
  assert.equal(unwrapReplyEnvelope(text), text);
  const html = highlightCode(code, "python");
  assert.ok(html.includes("tok-kw"), "keyword spans present");
  assert.ok(html.includes("tok-fn"), "function spans present");
});

check("legit bare fence WITH code is kept whole", () => {
  const text = "```\ndef tambah(a, b):\n    return a + b\n```";
  assert.equal(stripToolCallBlocks(text), text);
});

check("legit json fence with a non-tool-call object is kept whole", () => {
  const text = "```json\n{\"config\": true, \"count\": 3}\n```";
  assert.equal(stripToolCallBlocks(text), text);
  assert.equal(unwrapReplyEnvelope(text), text);
});

check("the word json inside a sentence is kept", () => {
  const text = "The format is json.\n```python\nprint(1)\n```";
  assert.equal(stripToolCallBlocks(text), text);
});

check("a lone 'json' line not next to a fence is kept when prose follows", () => {
  const text = "json\nSome prose about the format.";
  assert.equal(stripToolCallBlocks(text), text);
});

check("unclosed json fence is still hidden while streaming", () => {
  assert.equal(stripToolCallBlocks("```json\n{\"tool_calls\": ["), "");
  assert.equal(streamingReplyDisplay("```json\n{\"tool_calls\": ["), "");
});

check("bare tool-call JSON (no fence) is still stripped", () => {
  assert.equal(stripToolCallBlocks('{"tool_calls": [{"name": "write_file"}]}'), "");
});

check("fixed output has no 'json' text part and no empty code part", () => {
  const fixed = stripToolCallBlocks(msg2053);
  const parts = splitParts(fixed);
  assert.ok(!parts.some((p) => p.trim() === "json"), "no json text part");
  assert.ok(!parts.some((p, i) => i % 3 === 2 && p.trim() === ""), "no empty code part");
});

check("render-time strip (MessageView stripFences): old stored artifact displays empty", () => {
  // MessageView renders stored content as-is, so a message saved before
  // the save-time strip was fixed keeps showing its artifact. The render
  // path therefore applies the same strip for assistant messages
  // (stripFences prop, ChatPanel assistant branch only — user messages
  // are never stripped). This test pins the function that path calls.
  assert.equal(stripToolCallBlocks(msg2053), "");
  assert.equal(stripToolCallBlocks(stripToolCallBlocks(msg2053)), "");
  // ...while a normal assistant reply still renders untouched.
  const reply = "Berikut scriptnya:\n```python\ndef tambah(a, b):\n    return a + b\n```\nSemoga membantu!";
  assert.equal(stripToolCallBlocks(reply), reply);
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
