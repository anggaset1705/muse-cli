// Native live editing and non-forcing auto-scroll regression.
import assert from "node:assert/strict";
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const root = "C:/muse-tauri";
const diff = await import(pathToFileURL(root + "/src/lib/liveEditDiff.ts").href);
const scroll = await import(pathToFileURL(root + "/src/lib/scrollFollow.ts").href);
let checks = 0;
function check(name, fn) {
  try { fn(); checks++; console.log("PASS", name); }
  catch(e){ console.error("FAIL", name, e); process.exitCode = 1; }
}
check("added line highlighted on actual new content", () => {
  const result=diff.changedLineMarkers("a\nb\n", "a\nb\nc\n");
  assert.deepEqual(result.added,[3]);
  assert.deepEqual(result.modified,[]);
});
check("replacement line highlighted", () => {
  const r=diff.changedLineMarkers("def f():\n    return 1\n", "def f():\n    return 2\n");
  assert.deepEqual(r.modified,[2]); assert.equal(r.removed,1);
});
check("removal counted without touching unchanged line", () => {
  const r=diff.changedLineMarkers("alpha\nremove\nomega", "alpha\nomega");
  assert.deepEqual(r.added,[]);assert.deepEqual(r.modified,[]);assert.equal(r.removed,1);
});
check("multiple edits preserve unchanged intervening lines", () => {
  const r=diff.changedLineMarkers("a\none\nc\ntwo\ne","a\nONE\nc\nTWO\ne");
  assert.deepEqual(r.modified,[2,4]);
});
check("identical source has no misleading markers", () => {
  const r=diff.changedLineMarkers("import os\n", "import os\n");
  assert.equal(r.removed,0);assert.deepEqual(r.modified,[]);assert.deepEqual(r.added,[]);
});
check("user upward scroll pauses even while code grows",()=>{
  const m={scrollHeight:3000,clientHeight:300,scrollTop:1100};
  assert.equal(scroll.reconcileScrollFollow(true,true,m,64),false);
  assert.equal(scroll.reconcileScrollFollow(false,false,m,64),false);
  assert.equal(scroll.reconcileScrollFollow(false,false,{...m,scrollTop:2700},64),false);
  assert.equal(scroll.reconcileScrollFollow(false,true,{...m,scrollTop:2670},64),false);
  assert.equal(scroll.reconcileScrollFollow(false,true,{...m,scrollTop:2699},64),true);
  assert.equal(scroll.reconcileScrollFollow(true,false,m,64),true);
});
check("near-bottom tiny upward scroll stays paused",()=>{
  assert.equal(scroll.reconcileScrollFollow(false,true,{scrollHeight:1000,clientHeight:200,scrollTop:785},80),false);
});
const app=fs.readFileSync(root+"/src/App.tsx","utf8");
const workspace=fs.readFileSync(root+"/src/components/WorkspacePanel.tsx","utf8");
const editor=fs.readFileSync(root+"/src/components/CodeEditor.tsx","utf8");
const chat=fs.readFileSync(root+"/src/components/ChatPanel.tsx","utf8");
const view=fs.readFileSync(root+"/src/components/MessageView.tsx","utf8");
check("actual existing-file edit goes to editor and opens Files",()=>{
 assert.match(app,/case "tool_edit_begin"/);assert.match(app,/case "tool_edit_before"/);
 assert.match(app,/liveEditTargetRef.current\?\.path === ev.path/);
 assert.match(app,/setWorkspaceTab\("files"\)/);assert.match(app,/setRightCollapsed\(false\)/);
});
check("new-file code card remains in chat",()=>{
 assert.match(app,/content: \(actions\[i\]\.content \?\? ""\) \+ \(ev.text \?\? ""\)/);
 assert.match(chat,/!deferCode \? <LiveCode/);
});
check("preview is read-only and cannot overwrite unsaved editor buffer",()=>{
 assert.match(workspace,/liveFileEdit/);assert.match(workspace,/readOnly/);
 assert.match(workspace,/changedLineMarkers/);assert.match(editor,/EditorState\.readOnly\.of\(readOnly\)/);
 assert.match(editor,/if \(!readOnly\) onChange\(v\)/);
});
check("actual changed lines have distinctive decorations",()=>{
 assert.match(editor,/cm-live-added-line/);assert.match(editor,/cm-live-modified-line/);
 assert.match(workspace,/liveEdit\.diff/);
});
check("three scrolling surfaces have manual pause",()=>{
 assert.match(chat,/onWheel=\{handleChatWheel\}/);
 assert.match(chat,/if \(!followChatRef\.current \|\| frame\) return;/);
 assert.match(chat,/overflowAnchor: "none"/);assert.match(view,/overflowAnchor: "none"/);
 assert.match(editor,/if \(e\.deltaY < 0\) followingRef\.current = false/);
 assert.match(editor,/reconcileScrollFollow\(followingRef\.current, manualRef\.current/);
});
check("thinking stays running until tool result before final message",()=>{
 assert.match(app,/editingTurn = true/);assert.match(app,/if \(!editingTurn\) setStreamingText/);
 assert.match(app,/status: isFailure \? "failed" : "applied"/);
 assert.match(chat,/function ThinkingCard/);
});
console.log(checks+" SMART SCROLL + LIVE EDIT TESTS PASS");
