// Regression replay from actual browser DOM source, plus code scroll follow.
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import {pathToFileURL} from "node:url";
const backend="C:/muse-rust-backend/crates/muse-browser/src/dom.rs";
const frontend="C:/muse-tauri/src/components/MessageView.tsx";
const source=fs.readFileSync(backend,"utf8");
const view=fs.readFileSync(frontend,"utf8");
const scroll=await import(pathToFileURL("C:/muse-tauri/src/lib/scrollFollow.ts").href);
const start=source.indexOf("const replyText = node => {");
const end=source.indexOf("\n  };",start);
assert.ok(start>0 && end>start);
const js=source.slice(start,end+5);
const replyText=vm.runInNewContext("(()=>{"+js+";return replyText;})()",{Node:{TEXT_NODE:3,ELEMENT_NODE:1}});
let pass=0,fail=0;
function check(label,fn){try{fn();pass++;console.log("PASS "+label)}catch(e){fail++;console.log("FAIL "+label+": "+e.message.split("\n")[0])}}
const text=(s)=>({nodeType:3,nodeValue:s});
const bubbleWithCode=(code)=>{
 const pre={nodeType:1,tagName:"PRE",querySelector:(sel)=>sel==="code"?code:null,childNodes:[code],getAttribute:()=>null};
 return {nodeType:1,tagName:"DIV",childNodes:[pre],getAttribute:()=>null};
};
check("Rendered Python source keeps visually distinct lines from block syntax spans",()=>{
 const raw="import tkinter as tkBG = '#e0e5ec'TEXT = '#b5b6470'";
 const visible="import tkinter as tk\nBG = '#e0e5ec'\nTEXT = '#b5b6470'";
 const code={nodeType:1,tagName:"CODE",className:"language-python",innerText:visible,textContent:raw,getAttribute:()=>null};
 const result=replyText(bubbleWithCode(code));
 assert.ok(result.includes(visible),"Got flattened source: "+result.slice(0,110));
 assert.ok(result.includes("\x60\x60\x60python\n"));
 assert.ok(!result.includes(raw));
});
check("Syntax-highlighting spans without display-only newlines preserve exact source",()=>{
 const code={nodeType:1,tagName:"CODE",className:"language-rust",textContent:"fn main() {\n  println!(\"hi\");\n}",innerText:"fn main() {\n  println!(\"hi\");\n}",getAttribute:()=>null};
 assert.ok(replyText(bubbleWithCode(code)).includes("fn main() {\n  println!"));
});
check("Code extracted before innerText available keeps textContent fallback",()=>{
 const code={nodeType:1,tagName:"CODE",className:"language-json",textContent:'{\n  "ok": true\n}',getAttribute:()=>null};
 assert.ok(replyText(bubbleWithCode(code)).includes('{\n  "ok": true\n}'));
});
check("Normal assistant paragraphs outside code remain untouched",()=>{
 const b={nodeType:1,tagName:"DIV",childNodes:[text("Selesai."),{nodeType:1,tagName:"BR",getAttribute:()=>null,childNodes:[]},text("Berhasil.")],getAttribute:()=>null};
 assert.equal(replyText(b),"Selesai.\nBerhasil.\n");
});
check("Markdown CodeBlock owns scrollable pre and follows growth",()=>{
 assert.match(view,/const codePreRef = useRef<HTMLPreElement>\(null\)/);
 assert.match(view,/ref=\{codePreRef\}/);
 assert.match(view,/useLayoutEffect\(\(\) => \{[\s\S]*?scrollTop\s*=\s*el\.scrollHeight[\s\S]*?\},\s*\[code,\s*followLiveCode\]\)/);
});
check("User scrolling upward pauses; returning to bottom resumes",()=>{
 assert.match(view,/onWheel=\{/);
 assert.match(view,/onScroll=\{onCodeScroll\}/);
 const r=scroll.reconcileScrollFollow;
 assert.equal(r(true,false,{scrollHeight:2300,clientHeight:420,scrollTop:0},32),true);
 assert.equal(r(true,true,{scrollHeight:2300,clientHeight:420,scrollTop:0},32),false);
 assert.equal(r(false,false,{scrollHeight:2300,clientHeight:420,scrollTop:0},32),false);
 assert.equal(r(false,true,{scrollHeight:2300,clientHeight:420,scrollTop:1880},32),true);
});
check("Copy stays original full source",()=>{
 assert.match(view,/navigator\.clipboard\.writeText\(code\)/);
});
check("Nested code scroll is isolated from outer chat scroll follow",()=>{
 const chat=fs.readFileSync("C:/muse-tauri/src/components/ChatPanel.tsx","utf8");
 assert.match(view,/onWheel=\{\(e\) => \{\s*e\.stopPropagation\(\)/);
 assert.match(view,/onTouchMove=\{\(e\) => \{\s*e\.stopPropagation\(\)/);
 assert.match(chat,/onWheel=\{\(e\) => \{\s*e\.stopPropagation\(\)/);
});console.log("RESULT "+pass+" passed, "+fail+" failed");
process.exitCode=fail?1:0;
