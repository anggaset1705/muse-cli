// Regression: Muse edits in THE original Files CodeMirror panel, not
// a detached preview card; visual playback only uses real native source.
import assert from "node:assert/strict";
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const root = "C:/muse-tauri";
const { planInlineEdit, inlineEditFrame } = await import(pathToFileURL(root + "/src/lib/liveEditPlayback.ts").href);
let count = 0;
function check(name, run) {
  run();
  console.log("PASS", name);
  count++;
}
function exactReplay(before, after) {
  const plan = planInlineEdit(before, after);
  assert.equal(inlineEditFrame(plan, 0).text, before);
  assert.equal(inlineEditFrame(plan, plan.work).text, after);
  assert.equal(inlineEditFrame(plan, plan.work + 1000).text, after);
  for (let i = 0; i <= plan.work; i += Math.max(1, Math.floor(plan.work / 45))) {
    const frame=inlineEditFrame(plan,i);
    assert.ok(frame.text.length < Math.max(before.length, after.length) + 1000);
    assert.ok(frame.focusLine === null || (frame.focusLine >= 1 && Number.isFinite(frame.focusLine)));
  }
  return plan;
}
check("same source has no fake changes", () => {
  const p=exactReplay("print(1)\n","print(1)\n");
  assert.equal(p.hunks.length,0);
});
check("replacement happens in middle without hiding top or bottom", () => {
  const a="import tkinter as tk\n\n\ndef f():\n    return 1\n\n\nprint('end')\n";
  const b="import tkinter as tk\n\n\ndef f():\n    return 42\n\n\nprint('end')\n";
  const plan=exactReplay(a,b);
  assert.equal(plan.hunks.length,1);
  const middle=inlineEditFrame(plan,Math.min(6,plan.work-1)).text;
  assert.ok(middle.startsWith("import tkinter as tk"));
  assert.ok(middle.includes("print('end')"), "untouched source must remain");
  assert.equal(middle.includes("return 1"),false, "old modified line removed where editing");
});
check("multiple separated code edits animate one hunk at a time", () => {
  const a="HEADER\nold_one()\nUNCHANGED\nold_two()\nFOOTER\n";
  const b="HEADER\nnew_one()\nUNCHANGED\nnew_two()\nFOOTER\n";
  const plan=exactReplay(a,b);
  assert.equal(plan.hunks.length,2);
  const first=inlineEditFrame(plan,plan.hunks[0].inserted.length).text;
  assert.ok(first.includes("new_one()"));
  assert.ok(first.includes("old_two()"),"second original line still visible until Muse reaches it");
  assert.ok(first.includes("UNCHANGED"));
});
check("new function inserted exactly in the middle", () => {
  const a="import os\n\ndef old():\n    pass\n";
  const b="import os\n\ndef new():\n    return 3\n\ndef old():\n    pass\n";
  const plan=exactReplay(a,b);
  assert.ok(plan.hunks.some((h)=>h.beforeStart===h.beforeEnd));
});
check("deletion changes only exact removed region", () => {
  const p=exactReplay("one\nremove\nthree\n","one\nthree\n");
  assert.ok(p.hunks.some((h)=>h.inserted===""));
});
check("source with Windows CRLF bytes preserved",()=>{
  exactReplay("one\r\ntwo\r\nthree\r\n","one\r\nTWO\r\nthree\r\n");
});
check("unicode Python comments and emoji preserved",()=>{
  exactReplay("# beruang 🐻\nprint('lama')\n","# beruang 🐻\nprint('baru')\n");
});
check("new file and emptied file still have exact final content",()=>{
  exactReplay("","print(1)\n");
  exactReplay("print(1)\n","");
});
check("large file fallback avoids quadratic UI work",()=>{
  const before = Array.from({length:700},(_,i)=>"line "+i+"\n").join("");
  const after = before.replace("line 350\n","line 350 UPDATED\n");
  const plan=exactReplay(before,after);
  assert.equal(plan.hunks.length,1);
});
check("unmodified code is never retyped from scratch",()=>{
  const p=planInlineEdit("a\nb\nc\nd\ne\n","a\nB\nc\nD\ne\n");
  assert.ok(p.work < "a\nB\nc\nD\ne\n".length);
});
const panel=fs.readFileSync(root+"/src/components/WorkspacePanel.tsx","utf8");
const editor=fs.readFileSync(root+"/src/components/CodeEditor.tsx","utf8");
check("no second editor card; only the normal Files editor remains",()=>{
  assert.equal((panel.match(/<CodeEditor\b/g)||[]).length,1);
  assert.doesNotMatch(panel,/<section[^>]*data-live-edit-file=/);
  assert.match(panel,/ref=\{editorCardRef\}/);
});
check("the original editor is automatically opened from native edit",()=>{
  assert.match(panel,/setOpenFile\(\{ rel: nativePath, content: liveEdit\.before \}\)/);
  assert.match(panel,/setEdited\(liveEdit\.before\)/);
  assert.match(panel,/scrollIntoView\(\{ block: "nearest"/);
});
check("unsaved manual buffer is preserved and cannot be saved during replay",()=>{
  assert.match(panel,/if \(isDirty\) \{/);
  assert.match(panel,/setBlockedEditRevision\(liveEdit\.revision\)/);
  assert.match(panel,/disabled=\{inlinePreview\}/);
  assert.match(panel,/readOnly=\{inlinePreview\}/);
});
check("disk receipt verified before unlocking editable buffer",()=>{
  assert.match(panel,/read_workspace_file/);
  assert.match(panel,/res\.content\.replace/);
  assert.match(panel,/setVerifiedEditRevision\(revision\)/);
  assert.match(panel,/changedLineMarkers\(liveEdit\.before/);
});
check("editor follows actual changed line and respects user manual scroll",()=>{
  assert.match(editor,/followLine\?: number/);
  assert.match(editor,/EditorView\.scrollIntoView/);
  assert.match(editor,/if \(e\.deltaY < 0\) followingRef\.current = false/);
  assert.match(editor,/reconcileScrollFollow/);
});
console.log(count+" SINGLE EDITOR + IN-PLACE LIVE EDIT TESTS PASS");
