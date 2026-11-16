// Regression: ordinary assistant code must not pop into view at fence closure.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repo=path.resolve(path.dirname(fileURLToPath(import.meta.url)),"..");
const { typewriterStep }=await import(pathToFileURL(path.join(repo,"src/lib/typewriter.ts")).href);
const { stripToolCallBlocks, streamingReplyDisplay, unwrapReplyEnvelope }=await import(
 pathToFileURL(path.join(repo,"src/lib/replyEnvelope.ts")).href
);
const panelSource=fs.readFileSync(path.join(repo,"src/components/ChatPanel.tsx"),"utf8");
const messageViewSource=fs.readFileSync(path.join(repo,"src/components/MessageView.tsx"),"utf8");
assert.ok(panelSource.includes("typingTarget={typewriterTarget}"),"ChatPanel passes the full target");
assert.ok(panelSource.includes("typingTarget={isTypingTarget ? typingTarget : undefined}"),"MessageRow passes only while typing");
assert.ok(messageViewSource.includes("stripToolCallBlocks(content, typingTarget)"),"MessageView uses safe target hint");
console.log("PASS real React render path is wired to the validated target");
const F=String.fromCharCode(96).repeat(3);
const split=new RegExp(F+"(\\w*)\\n?");
function codeLen(text){const p=text.split(split);return p.length>=3?p[2].length:0;}

function replay(text, chunk=0){
 const events=[];
 if(!chunk){events.push([0,"delta",text]);events.push([200,"finish",""]);}
 else{
  for(let i=0;i<text.length;i+=chunk){events.push([Math.floor(i/chunk)*24,"delta",text.slice(i,i+chunk)]);}
  events.push([events.at(-1)[0]+100,"finish",""]);
 }
 let full="",saved="",displayed="",streaming=true,streamingText="",ei=0,prev=0,max=0,sawPartial=false,ticks=0;
 for(let t=0;t<30000;t+=16){
  while(ei<events.length && events[ei][0]<=t){
   const [,kind,data]=events[ei++];
   if(kind==="delta"){full+=data;streamingText=streamingReplyDisplay(full);}
   if(kind==="finish"){streaming=false;saved=unwrapReplyEnvelope(full);streamingText="";}
  }
  const target=streaming?streamingText:saved;
  if(displayed.length<target.length){const next=typewriterStep(target,displayed,streaming);if(next.length>displayed.length)displayed=next;}
  const typing=streaming || displayed.length<target.length;
  const render=typing?stripToolCallBlocks(displayed,target):stripToolCallBlocks(saved);
  const code=codeLen(render);
  max=Math.max(max,code-prev);
  if(code>30 && code<codeLen(text))sawPartial=true;
  prev=code;ticks++;
  if(!streaming && displayed.length>=saved.length && render===saved)break;
 }
 return {max,sawPartial,ticks,renderedCode:prev};
}
const original=fs.readFileSync(path.join(repo,"scripts/fixtures/msg-2212.txt"),"utf8");
const ordinaryBare=[
 "Contoh konfigurasi JavaScript:",F,
 ...Array.from({length:25},(_,i)=>"const pre"+i+" = "+i+";"),
 "const cfg = {",
 '  "actions": ["read", "write"],',
 '  "mode": "developer"',
 "};",
 "console.log(cfg.actions);",F
].join("\n");
const ordinaryJson=["Data JSON biasa:",F+"json","{",
 '  "name": "muse",',
 '  "description": "Normal JSON from assistant",',
 '  "count": 42,',
 '  "enabled": true',
 "}",F].join("\n");
const ordinaryBareJson=[
 "Contoh objek JSON biasa:",F,"{",
 ...Array.from({length:18},(_,i)=>'  "field'+i+'": "value'+i+'",'),
 '  "actions": "read-only",',
 '  "finished": true',
 "}",F
].join("\n");
const cases=[
 ["original-2212 single",original,0],
 ["ordinary bare actions single",ordinaryBare,0],
 ["ordinary bare actions chunked",ordinaryBare,24],
 ["ordinary json tag single",ordinaryJson,0],
 ["ordinary json tag chunked",ordinaryJson,16],
 ["ordinary bare JSON non-tool actions chunked",ordinaryBareJson,20]
];
let failed=0;
for(const [label,msg,chunk] of cases){
 const r=replay(msg,chunk);
 const passed=r.max<=12 && r.sawPartial;
 console.log((passed?"PASS ":"FAIL ")+label+" ticks="+r.ticks+" maxCodeJump="+r.max+" partial="+r.sawPartial);
 if(!passed)failed++;
}
const toolPayload=F+"json\n"+JSON.stringify({tool_calls:[{name:"run_command",arguments:{command:"echo secret"}}]})+"\n"+F;
assert.equal(stripToolCallBlocks(toolPayload,toolPayload),"");
assert.equal(streamingReplyDisplay(toolPayload),"");
assert.equal(stripToolCallBlocks(F+'json\n{"tool_calls": [', F+'json\n{"tool_calls": ['),"");
console.log("PASS tool-call JSON remains hidden with and without target hint");
console.log("RESULT failed="+failed);
if(failed)process.exit(1);
