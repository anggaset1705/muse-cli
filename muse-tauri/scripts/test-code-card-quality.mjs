import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
const root=path.resolve(import.meta.dirname,"..");
const mv=fs.readFileSync(path.join(root,"src/components/MessageView.tsx"),"utf8");
const cp=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
const css=fs.readFileSync(path.join(root,"src/App.css"),"utf8");
const checkerPath="C:/muse-rust-backend/crates/muse-agent/src/code_quality.rs";
const checker=fs.existsSync(checkerPath) ? fs.readFileSync(checkerPath,"utf8") : "";
let passed=0,failed=0;
function check(label,test) { try{test();passed++;console.log("PASS "+label)}catch(e){failed++;console.log("FAIL "+label+" "+e.message.split("\n")[0]);}}
check("all ordinary code cards have scroll-independent language/Copy header",()=>{
 assert.match(mv,/<div className="md-code-shell">[\s\S]*?<div className="md-pre-head">[\s\S]*?<\/div>[\s\S]*?<pre\s+ref=\{codePreRef\}[\s\S]*?className="md-pre md-code-scroll"/);
});
check("all Ran source cards have scroll-independent language/Copy header",()=>{
 assert.match(cp,/<div className="md-code-shell">[\s\S]*?<div className="md-pre-head">[\s\S]*?<\/div>[\s\S]*?<pre[\s\S]*?ref=\{codePreRef\}/);
});
check("no nested header inside scrolling pre",()=>{
 assert.doesNotMatch(mv,/<pre className="md-pre[^"]*">[\s\S]*?<div className="md-pre-head">/);
 assert.doesNotMatch(cp,/<pre[\s\S]*?ref=\{codePreRef\}[\s\S]*?>\s*<div className="md-pre-head">/);
});
check("all code blocks remain bounded and scrollable",()=>{
 assert.match(css,/\.md-body \.md-code-shell \.md-code-scroll\s*\{[^}]*max-height/s);
 assert.match(css,/\.md-body \.md-code-shell \.md-code-scroll\s*\{[^}]*overflow:\s*auto/s);
});
check("sticky head has independent layout and dark/light opaque backgrounds",()=>{
 assert.match(css,/\.md-body \.md-pre-head\s*\{[^}]*position:\s*sticky[^}]*top:\s*0[^}]*z-index:/s);
 assert.match(css,/\.theme-light \.md-body \.md-code-shell/);
 assert.match(css,/\.theme-light \.md-body \.md-pre-head/);
});
check("Ran parent does not clip outer sticky header",()=>{
 assert.match(cp,/className="mt-2 overflow-visible rounded-lg border/);
});
check("code and Copy keep original unformatted source",()=>{
 assert.match(mv,/highlightCode\(code,\s*effLang\)/);
 assert.match(mv,/navigator\.clipboard\.writeText\(code\)/);
 assert.match(cp,/highlightCode\(displayed,\s*codeLang\)/);
 assert.match(cp,/navigator\.clipboard\.writeText\(content\)/);
});
check("existing ordered prose-then-code and slow typewriter untouched",()=>{
 assert.match(cp,/ranCodeSequence\(/);
 assert.match(cp,/liveContentStep\(target,\s*cur\)/);
});
check("syntax quality module guards Python, JSON and flattened files",()=>{
 assert.match(checker,/ast\.parse/);
 assert.match(checker,/serde_json::from_str/);
 assert.match(checker,/source\.lines\(\)/);
});
console.log("RESULT="+passed+" passed "+failed+" failed");
if(failed)process.exitCode=1;
