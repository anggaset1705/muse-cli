import assert from "node:assert/strict";
import fs from "node:fs";
import { pathToFileURL } from "node:url";
const root = "C:/muse-tauri";
const { isLiveCodeGrowth } = await import(pathToFileURL(root + "/src/lib/codeScrollFollow.ts").href);
const { reconcileScrollFollow } = await import(pathToFileURL(root + "/src/lib/scrollFollow.ts").href);
let count=0;
function test(label, fn){ fn();console.log("PASS",label);count++; }
test("historical archived code is not scrolled to its end on first mount", () => {
  assert.equal(isLiveCodeGrowth(null,"print('old')\n",false),false);
});
test("live assistant code auto follows during typing",()=>{
  assert.equal(isLiveCodeGrowth(null,"print(",true),true);
  assert.equal(isLiveCodeGrowth("print(","print('hello')",true),true);
});
test("code keeps auto following after streaming flag stops but text still grows",()=>{
  assert.equal(isLiveCodeGrowth("print(1)\n", "print(1)\nprint(2)",false),true);
});
test("code content replacement does not unexpectedly jump the card",()=>{
  assert.equal(isLiveCodeGrowth("first snippet", "new snippet",false),false);
});
test("code follow remains active while user scrolls OUTER chat upward",()=>{
  const chatTop={scrollHeight:2200,clientHeight:420,scrollTop:700};
  const codeTop={scrollHeight:850,clientHeight:300,scrollTop:550};
  const chatFollow=reconcileScrollFollow(true,true,chatTop,80);
  let codeFollow=true;
  assert.equal(chatFollow,false);
  codeFollow=reconcileScrollFollow(codeFollow,false,codeTop,32);
  assert.equal(codeFollow,true);
  assert.equal(isLiveCodeGrowth("old\n","old\nmore\n",false),true);
});
test("only a user scroll inside CODE card pauses its own auto follow",()=>{
  const inside={scrollHeight:900,clientHeight:300,scrollTop:300};
  let codeFollow=reconcileScrollFollow(true,true,inside,32);
  assert.equal(codeFollow,false);
  codeFollow=reconcileScrollFollow(codeFollow,false,{...inside,scrollHeight:1100},32);
  assert.equal(codeFollow,false);
  codeFollow=reconcileScrollFollow(codeFollow,true,{scrollHeight:1100,clientHeight:300,scrollTop:799},32);
  assert.equal(codeFollow,true);
});
const mv=fs.readFileSync(root+"/src/components/MessageView.tsx","utf8");
const cp=fs.readFileSync(root+"/src/components/ChatPanel.tsx","utf8");
test("markdown code has inner scroll lifecycle independent of chat",()=>{
  assert.match(mv,/isLiveCodeGrowth\(previous, code, followLiveCode\)/);
  assert.match(mv,/new MutationObserver\(requestFollow\)/);
  assert.match(mv,/new ResizeObserver\(requestFollow\)/);
  assert.match(mv,/el\.scrollTop = el\.scrollHeight/);
  assert.match(mv,/if \(followCodeRef\.current && codeGrowingRef\.current\)/);
});
test("only code-card wheel/touch events pause inner follow",()=>{
  assert.match(mv,/e\.stopPropagation\(\); \/\/ Do not pause the outer chat follow/);
  assert.match(mv,/if \(e\.deltaY < 0\) followCodeRef\.current = false/);
  assert.match(cp,/onWheel=\{handleChatWheel\}/);
  assert.match(cp,/const followChatRef = useRef\(true\)/);
});
test("Ran code card is still independent and keeps its existing typewriter",()=>{
  assert.match(cp,/function LiveCode\(/);
  assert.match(cp,/const followCodeRef = useRef\(true\)/);
  assert.match(cp,/useLayoutEffect\(\(\) => \{\s*const el = codePreRef\.current;/);
  assert.match(cp,/liveContentStep\(target, cur\)/);
});
console.log(count+" CHAT CODE AUTO-FOLLOW TESTS PASS");
