import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),"..");
const {highlightToolOutput,highlightCodeDiff}=await import(pathToFileURL(path.join(root,"src/lib/toolOutputHighlight.ts")).href);
let tests=0;
function test(label,fn){fn();console.log("PASS",label);tests++;}
function unrender(html){
  return html.replace(/<\/?span(?:\s+[^>]*)?>/g,"")
    .replace(/&quot;/g,'"').replace(/&gt;/g,">")
    .replace(/&lt;/g,"<").replace(/&amp;/g,"&");
}
const read="File: kalkulator_sederhana.py\nLines: 1-208/208\ncontent_sha256: B164B70D6486C6CF72B74489A9467807E0C6099609E3217BDC3684103A6B9816\n\n   1: # Kalkulator Sederhana - UI Neumorphism\n   2: import tkinter as tk\n   3: def tambah(a, b):\n   4:     return a + b\n   5: BG = \"#E0E5EC\"\n";
test("read_file numbers, Python keywords, comments and strings have colour",()=>{
  const html=highlightToolOutput("read_file",read,"kalkulator_sederhana.py","done");
  assert.match(html,/class="tok-num"[^>]*>\s*1:/);
  assert.match(html,/class="tok-kw">import<\/span>/);
  assert.match(html,/class="tok-kw">def<\/span>/);
  assert.match(html,/class="tok-com"># Kalkulator/);
  assert.match(html,/class="tok-str">&quot;#E0E5EC&quot;<\/span>/);
  assert.match(html,/class="tok-key">File<\/span>/);
  assert.equal(unrender(html),read);
});
test("edit_file success coloured with unmodified receipt text",()=>{
  const raw="edited kalkulator_sederhana.py (19 replacement(s))";
  const html=highlightToolOutput("edit_file",raw,"kalkulator_sederhana.py","done");
  assert.match(html,/class="tok-type">edited<\/span>/);
  assert.match(html,/class="tok-str"> kalkulator_sederhana.py/);
  assert.equal(unrender(html),raw);
});
test("write_file status coloured even without source card",()=>{
  const raw="wrote proyek_saya.py (430 chars)";
  const html=highlightToolOutput("write_file",raw,"proyek_saya.py","done");
  assert.match(html,/class="tok-type">wrote<\/span>/);
  assert.equal(unrender(html),raw);
});
test("run_command Python source and exit status coloured without changing log",()=>{
  const raw="exit_code=0\nstdout:\ndef hitung(a):\n    return a*2\nstderr:\n";
  const html=highlightToolOutput("run_command",raw,undefined,"done");
  assert.match(html,/class="tok-key">exit_code<\/span>/);
  assert.match(html,/class="tok-kw">def<\/span>/);
  assert.match(html,/class="tok-kw">return<\/span>/);
  assert.equal(unrender(html),raw);
});
test("errors are red, never green",()=>{
  const raw="exit_code=1\nstderr:\nTraceback (most recent call last):\nModuleNotFoundError: No module named 'cv2'";
  const html=highlightToolOutput("run_command",raw,undefined,"error");
  assert.match(html,/class="tok-err">/);
  assert.doesNotMatch(html,/class="tok-type">/);
  assert.equal(unrender(html),raw);
});
test("all output preserves CRLF, Unicode, whitespace, ampersands",()=>{
  const raw="File: a.py\r\n   1: print('beruang 🐻 & teman')\r\n   2: a = 2 < 5\r\n";
  const html=highlightToolOutput("read_file",raw,"a.py","done");
  assert.equal(unrender(html),raw);
});
test("escape-first syntax highlighting cannot execute injected markup",()=>{
  const malicious="<img src=x onerror=alert(1)><svg onload=alert(2)>&\"'";
  const html=highlightToolOutput("read_file","   1: "+malicious,"danger.py","done");
  assert.equal(unrender(html),"   1: "+malicious);
  assert.doesNotMatch(html,/<img\b|<svg\b/);
});
test("diff lines coloured by addition and removal, preserving exact text",()=>{
  const raw="--- a/main.py\n+++ b/main.py\n@@ -2,2 +2,2 @@\n- BG = '#eee'\n+ BG = '#2563EB'\n  return 5\n";
  const html=highlightCodeDiff(raw,"main.py");
  assert.match(html,/class="tok-type">\+<\/span>/);
  assert.match(html,/class="tok-err">-<\/span>/);
  assert.match(html,/class="tok-kw">return<\/span>/);
  assert.equal(unrender(html),raw);
});
test("no-code ordinary diagnostic text stays escaped and does not invent content",()=>{
  const raw="Searching the current workspace\nThere are no files to modify.\n[ok]";
  const html=highlightToolOutput("list_files",raw,undefined,"done");
  assert.equal(unrender(html),raw);
  assert.doesNotMatch(html,/<script\b/);
});
const chat=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
const workspace=fs.readFileSync(path.join(root,"src/components/WorkspacePanel.tsx"),"utf8");
const css=fs.readFileSync(path.join(root,"src/App.css"),"utf8");
test("all Ran previews, outputs and legacy JSON plan use syntax-coloured renderer",()=>{
  assert.match(chat,/highlightToolOutput\(a\.tool, visibleResult\.slice/);
  assert.match(chat,/highlightToolOutput\(a\.tool, a\.preview\.slice/);
  assert.match(chat,/highlightCodeDiff\(a\.preview\.slice/);
  assert.match(chat,/highlightCode\(JSON\.stringify\(actions, null, 2\), "json"\)/);
});
test("right-hand code diffs and terminal output use same renderer",()=>{
  assert.match(workspace,/highlightCodeDiff\(liveEdit\.diff\.slice/);
  assert.match(workspace,/highlightCodeDiff\(c\.diff\.slice/);
  assert.match(workspace,/highlightToolOutput\("run_command", termOutput/);
});
test("dark and light themes both colour output tokens and errors",()=>{
  assert.match(css,/\.md-body \.tok-err/);
  assert.match(css,/\.theme-light \.md-body \.tok-err/);
  assert.match(chat,/className="md-body max-h-48/);
  assert.match(workspace,/className="md-body .*whitespace-pre-wrap/);
});
console.log(tests+" TOOL OUTPUT SYNTAX COLOR TESTS PASSED");
