// 17 November: matching, bounded pacing across chat and existing-file edits.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ts = await import(pathToFileURL(path.join(root, "src/lib/typewriter.ts")).href);
const js = await import(pathToFileURL(path.join(root, "src/lib/typewriter.js")).href);
const { planInlineEdit, inlineEditFrame } = await import(
  pathToFileURL(path.join(root, "src/lib/liveEditPlayback.ts")).href
);
const panel = fs.readFileSync(path.join(root, "src/components/WorkspacePanel.tsx"), "utf8");
let passed = 0;
function check(label, action) {
  action();
  console.log("PASS", label);
  passed++;
}
function chatTicks(full) {
  let cur = "";
  let ticks = 0;
  while (cur !== full && ticks < 15000) {
    const next = ts.typewriterStep(full, cur, false);
    assert.ok(next.length > cur.length && next.length <= cur.length + 4);
    cur = next;
    ticks++;
  }
  assert.equal(cur, full);
  return ticks;
}
function editorTicks(before, after) {
  const plan = planInlineEdit(before, after);
  let work = 0;
  let ticks = 0;
  while (work < plan.work && ticks < 15000) {
    const step = ts.typewriterStepSize(plan.work - work);
    assert.ok(step >= 2 && step <= 4);
    work = Math.min(plan.work, work + step);
    const frame = inlineEditFrame(plan, work);
    assert.ok(frame.text.startsWith("import tkinter as tk\n"));
    assert.ok(frame.text.includes("def hitung"), "other function must remain visible");
    ticks++;
  }
  assert.equal(inlineEditFrame(plan, work).text, after, "real file result preserved");
  return { ticks, plan };
}

check("all reveal surfaces share same 24ms cadence", () => {
  assert.equal(ts.TYPEWRITER_TICK_MS, 24);
  assert.equal(js.TYPEWRITER_TICK_MS, ts.TYPEWRITER_TICK_MS);
  assert.match(panel, /import \{ typewriterStepSize, TYPEWRITER_TICK_MS \} from "@\/lib\/typewriter"/);
  assert.match(panel, /work \+ typewriterStepSize\(livePlan\.work - work\)/);
  assert.match(panel, /\}, TYPEWRITER_TICK_MS\);/);
  assert.doesNotMatch(panel, /Math\.min\(120,\s*Math\.max\(12/);
});
check("TypeScript and shipped JavaScript implementations agree", () => {
  for (const gap of [1, 2, 90, 600, 601, 2000, 2001, 10000]) {
    assert.equal(ts.typewriterStepSize(gap), js.typewriterStepSize(gap));
  }
  assert.deepEqual([90, 900, 3500].map(ts.typewriterStepSize), [2, 3, 4]);
});
check("short plain chat is faster but remains readable", () => {
  const reply = "Terima kasih. File yang diminta akan saya baca sebelum perubahan dilakukan.";
  const ticks = chatTicks(reply);
  const durationMs = ticks * ts.TYPEWRITER_TICK_MS;
  assert.ok(durationMs >= 700 && durationMs <= 1600, "duration: " + durationMs);
});
check("existing file edit is noticeably slower than old 12-120 chars per 32ms", () => {
  const before = "import tkinter as tk\n\ndef hitung(x, y):\n    return x + y\n";
  const after = before + ("\n# Biru profesional untuk tombol sama dengan\n").repeat(26);
  const { ticks, plan } = editorTicks(before, after);
  const durationMs = ticks * ts.TYPEWRITER_TICK_MS;
  const oldStep = Math.min(120, Math.max(12, Math.ceil(plan.work / 75)));
  const oldDuration = Math.ceil(plan.work / oldStep) * 32;
  assert.ok(durationMs > oldDuration * 2.5, "edit still too fast: " + durationMs);
  assert.ok(durationMs < 30000, "edit too slow: " + durationMs);
});
check("normal code card also follows the same bounded 2/3/4 pacing", () => {
  const target = "print(123)\n".repeat(100);
  let current = "";
  let ticks = 0;
  while (current.length < target.length && ticks < 15000) {
    const next = ts.liveContentStep(target, current);
    assert.ok(next.length - current.length <= 4);
    current = next;
    ticks++;
  }
  assert.equal(current, target);
});
check("pacing never alters prepared code, approval or Save operation", () => {
  const source = fs.readFileSync(path.join(root, "src/lib/liveEditPlayback.ts"), "utf8");
  assert.match(source, /if \(plan\.work === 0 \|\| workDone >= plan\.work\)/);
  assert.match(panel, /read_workspace_file/);
  assert.match(panel, /disabled=\{inlinePreview\}/);
  assert.match(panel, /setVerifiedEditRevision\(revision\)/);
});
console.log(passed + " BALANCED TYPING PACE TESTS PASS");
