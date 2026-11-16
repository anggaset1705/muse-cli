// UI regression for ordered prose -> Ran code and sticky language/Copy.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {fileURLToPath,pathToFileURL} from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)),"..").replaceAll("\\","/");
const chat=fs.readFileSync(root+"/src/components/ChatPanel.tsx","utf8");
const css=fs.readFileSync(root+"/src/App.css","utf8");
const view=fs.readFileSync(root+"/src/components/MessageView.tsx","utf8");
const helperPath=root+"/src/lib/ranSequence.ts";
let count=0,fail=0;
function check(name,fn){try{fn();count++;console.log("PASS "+name)}catch(e){fail++;console.log("FAIL "+name+": "+e.message.split("\n")[0]);}}
check("sequencing decisions are explicit and independently testable",()=>{
 assert.equal(fs.existsSync(helperPath),true);
});
if(fs.existsSync(helperPath)){
 const {ranCodeSequence}=await import(pathToFileURL(root+"/src/lib/ranSequence.ts").href);
 check("current active turn waits before code",()=>{
  assert.deepEqual(ranCodeSequence(true,true,true),{animate:true,defer:true});
  assert.deepEqual(ranCodeSequence(true,true,false),{animate:true,defer:true});
  assert.deepEqual(ranCodeSequence(true,false,true),{animate:true,defer:true});
  assert.deepEqual(ranCodeSequence(true,false,false),{animate:true,defer:false});
 });
  const {typewriterStep,liveContentStep}=await import(pathToFileURL(root+"/src/lib/typewriter.ts").href);
 check("typewriter finishes prose before code starts",()=>{
  const prose="Selesai. File Python kalkulator sudah dibuat, lengkap dengan fitur.";
  const code="import tkinter as tk\n\ndef main():\n    print('hello')\n";
  let visibleText="",visibleCode="",lastTextTick=-1,firstCodeTick=-1,tick=0;
  while((visibleText!==prose || visibleCode!==code) && tick<3000){
   const timing=ranCodeSequence(true,false,visibleText!==prose);
   if(visibleText!==prose){
    visibleText=typewriterStep(prose,visibleText,false);
    if(visibleText===prose)lastTextTick=tick;
   }
   if(!timing.defer){
    const next=liveContentStep(code,visibleCode);
    assert.ok(next.length-visibleCode.length<=4);
    visibleCode=next;
    if(firstCodeTick===-1)firstCodeTick=tick;
   }
   tick++;
  }
  assert.equal(visibleText,prose);
  assert.equal(visibleCode,code);
  assert.ok(firstCodeTick>lastTextTick);
 });check("saved history never restarts or delays its code",()=>{
  assert.deepEqual(ranCodeSequence(false,false,false),{animate:false,defer:false});
  assert.deepEqual(ranCodeSequence(false,true,true),{animate:false,defer:false});
 });
}
check("MessageRow receives turn identity for code order",()=>assert.match(chat,/sequenceRanCode=\{/));
check("Ran card derives ordering from current turn text animation",()=>assert.match(chat,/ranCodeSequence\(sequenceRanCode,\s*isStreamingTarget,\s*isTypingTarget\)/));
check("Ran code not mounted during prose typing",()=>assert.match(chat,/!deferCode\s*\?\s*<LiveCode/));
check("post-prose Ran code still starts empty and types",()=>assert.match(chat,/live\s*\|\|\s*animateFromEmpty/));
check("Ran code still uses original liveContentStep",()=>assert.match(chat,/liveContentStep\(target,\s*cur\)/));
check("code header remains in its scrolling block with Copy action",()=>{
 assert.match(chat,/<div className="md-pre-head">[\s\S]{0,500}<button[\s\S]{0,160}onClick=\{copy\}/);
 assert.match(view,/<div className="md-pre-head">[\s\S]{0,250}<button[\s\S]{0,160}onClick=\{copy\}/);
});
check("code header sticks at top of scrolling code viewport",()=>{
 const cssRule=css.split(".md-body .md-pre-head {").at(-1)?.split("}")[0]||"";
 assert.match(cssRule,/position:\s*sticky/);
 assert.match(cssRule,/top:\s*0/);
 assert.match(cssRule,/z-index:\s*\d+/);
 assert.match(cssRule,/background:\s*#17171d/i);
});
check("sticky header respects light mode",()=>{
 assert.ok(css.includes(".theme-light .md-body .md-pre-head { color: #6e6e73; border-bottom-color: #e2e2e8; background: #f8f8fa; }"));
});
check("original code content is copied, not the displayed prefix",()=>{
 assert.match(chat,/navigator\.clipboard\.writeText\(content\)/);
 assert.match(view,/navigator\.clipboard\.writeText\(code\)/);
});
console.log("RESULT="+count+" PASS, "+fail+" FAIL");
if(fail)process.exitCode=1;