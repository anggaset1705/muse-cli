// Regression for slower typing, nested auto-scroll and hiding only the
// successful write_file receipt. Runs against REAL source (Node 24).
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),"..");
const typewriter=await import(pathToFileURL(path.join(root,"src/lib/typewriter.ts")).href);
const ran=await import(pathToFileURL(path.join(root,"src/lib/ranCard.ts")).href);
const typewriterJs=await import(pathToFileURL(path.join(root,"src/lib/typewriter.js")).href);
const scrollFile=path.join(root,"src/lib/scrollFollow.ts");
const scroll=fs.existsSync(scrollFile) ? await import(pathToFileURL(scrollFile).href) : {};
let passed=0,failed=0;
function check(label,fn){try{fn();passed++;console.log("  PASS "+label);}catch(e){failed++;console.log("  FAIL "+label+": "+e.message);}}

check("slower actual intervals and bounded tiers",()=>{
  assert.ok(typewriter.TYPEWRITER_TICK_MS >= 22 && typewriter.TYPEWRITER_TICK_MS<=40);
  assert.deepEqual([90,601,2001].map(typewriter.typewriterStepSize),[1,2,4]);
  let cur="";const target="x".repeat(1334);let ticks=0;
  while(cur.length<target.length && ticks<3000){
    const next=typewriter.liveContentStep(target,cur);
    assert.ok(next.length-cur.length>0 && next.length-cur.length<=4);
    cur=next;ticks++;
  }
  assert.equal(cur,target);
  const duration=ticks*typewriter.TYPEWRITER_TICK_MS;
  assert.ok(duration>=15000 && duration<=35000,"1334 chars in "+duration+"ms");
  console.log("    (1334 chars ~"+(duration/1000).toFixed(1)+"s)");
});
check("Vite-loaded JavaScript typewriter mirrors the TypeScript engine",()=>{
  assert.equal(typewriterJs.TYPEWRITER_TICK_MS,typewriter.TYPEWRITER_TICK_MS);
  for(const gap of [1,90,600,601,2000,2001,6000]){
    assert.equal(typewriterJs.typewriterStepSize(gap),typewriter.typewriterStepSize(gap));
  }
  assert.equal(typewriterJs.typewriterStep("abcdefghij","","true"),typewriter.typewriterStep("abcdefghij","","true"));
});
check("both real interval callbacks use the slower shared cadence",()=>{
  const source=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
  const found=source.match(/}, TYPEWRITER_TICK_MS\);/g) || [];
  assert.equal(found.length,2);
});check("write_file success receipt hidden but errors and other results kept",()=>{
  assert.equal(ran.shouldShowRanOutput("write_file","done","wrote kalkulator_sederhana.py (1334 chars)"),false);
  assert.equal(ran.shouldShowRanOutput("write_file","error","wrote kalkulator_sederhana.py (1334 chars)"),true);
  assert.equal(ran.shouldShowRanOutput("run_command","done","wrote kalkulator_sederhana.py (1334 chars)"),true);
  assert.equal(ran.shouldShowRanOutput("write_file","done","Permission denied"),true);
  assert.equal(ran.shouldShowRanOutput("write_file","done","wrote foo.py (1334 chars)\nWARNING: mismatch"),true);
});
check("chat auto-scroll wiring watches nested card mutations",()=>{
  const c=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
  assert.match(c,/new MutationObserver/);
  assert.match(c,/useLayoutEffect/);
  assert.match(c,/onWheel=\{handleChatWheel\}/);
  assert.match(c,/scrollTop\s*=\s*el\.scrollHeight/);
});
check("Ran code pre follows typing independently of chat scroll",()=>{
  const c=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
  assert.match(c,/ref=\{codePreRef\}/);
  assert.match(c,/codePreRef\.current/);
  assert.match(c,/onScroll=\{onCodeScroll\}/);
  assert.match(c,/liveContentStep\(target, cur\)/);
});
check("Ran card suppresses ONLY write-file receipt and keeps preview/errors",()=>{
  const c=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
  assert.match(c,/shouldShowRanOutput\(a\.tool, a\.status, a\.result\)/);
  assert.match(c,/a\.preview &&/);
  assert.match(c,/visibleResult &&/);
});
check("newly started assistant turn re-enables automatic following",()=>{
  const source=fs.readFileSync(path.join(root,"src/components/ChatPanel.tsx"),"utf8");
  assert.match(source,/if \(props\.streaming\) \{\s*followChatRef\.current = true;/);
});check("manual scroll stays paused; returning near bottom resumes",()=>{
  const r=scroll.reconcileScrollFollow;
  assert.equal(typeof r,"function");
  const metrics={scrollHeight:2000,clientHeight:300,scrollTop:100};
  assert.equal(r(true,false,metrics,64),true,"programmatic layout growth must not disable follow");
  assert.equal(r(true,true,metrics,64),false,"user scrolled up");
  assert.equal(r(false,false,metrics,64),false,"manual pause persists");
  assert.equal(r(false,true,{...metrics,scrollTop:1700},64),true,"user returned to bottom");
});
console.log("\n"+passed+" passed, "+failed+" failed");
process.exit(failed?1:0);