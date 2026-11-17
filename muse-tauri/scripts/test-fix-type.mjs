// Regression tests for the 13 Nov 2026 fix-type round (Bree's 22:12 live
// test: the code block appeared all at once, not live typewriting —
// even though the reply was coloured and the "json" + empty block were
// gone). Fixture scripts/fixtures/msg-2212.txt is the EXACT stored
// assistant message from that turn (read-only leveldb probe of Bree's
// PC), not a made-up sample. Run:
//   node scripts/test-fix-type.mjs
// It transpiles the REAL src/lib/typewriter.ts + src/lib/replyEnvelope.ts
// + src/lib/highlight.ts with the repo's tsc into a temp dir and drives
// them in Node, simulating the frontend event sequence faithfully
// (App streamingText + ChatPanel 16 ms chase + MessageRow isTyping gate).
// Exits non-zero on any failure.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fixtype-test-"));
const tmpTsconfig = path.join(tmp, "tsconfig.json");
fs.writeFileSync(
  tmpTsconfig,
  JSON.stringify({
    compilerOptions: {
      target: "es2022",
      module: "nodenext",
      moduleResolution: "nodenext",
      rootDir: path.join(repo, "src", "lib"),
      outDir: tmp,
      strict: true,
    },
    files: [
      path.join(repo, "src", "lib", "typewriter.ts"),
      path.join(repo, "src", "lib", "replyEnvelope.ts"),
      path.join(repo, "src", "lib", "highlight.ts"),
    ],
  }),
);
execFileSync(process.execPath, [path.join(repo, "node_modules", "typescript", "bin", "tsc"), "-p", tmpTsconfig], { stdio: "pipe" });
const { typewriterStep, TYPEWRITER_TICK_MS } = await import(pathToFileURL(path.join(tmp, "typewriter.js")).href);
const { streamingReplyDisplay, unwrapReplyEnvelope, stripToolCallBlocks } = await import(pathToFileURL(path.join(tmp, "replyEnvelope.js")).href);
const { highlightCode, detectLang } = await import(pathToFileURL(path.join(tmp, "highlight.js")).href);

const msg2212 = fs.readFileSync(path.join(repo, "scripts", "fixtures", "msg-2212.txt"), "utf8");

let passed = 0;
let failed = 0;
function check(name, fn) {
  try {
    fn();
    passed++;
    console.log("  PASS " + name);
  } catch (e) {
    failed++;
    console.log("  FAIL " + name + ": " + (e && e.message ? e.message : e));
  }
}

// MessageView's fence split, mirrored here (no React in Node).
function splitParts(content) {
  return content.split(/```(\w*)\n?/);
}
// Length of the code part (parts[2]) of the first fenced block in `content`.
function codeLen(content) {
  const parts = splitParts(content);
  return parts.length >= 3 ? parts[2].length : 0;
}

/**
 * Faithful simulation of the frontend live path for one turn.
 *
 * Models App.tsx (streamingText state + finish() saving) and ChatPanel.tsx
 * (displayed state chased by a 16 ms interval; typewriterTarget =
 * streaming ? streamingText : saved content; MessageRow renders
 * `displayed` while isTyping, else the saved content).
 *
 * `events` is a list of [timeMs, kind, payload] in wall-clock order:
 * - ["delta", fullText]: assistant_delta — App appends to `full` and sets
 *   streamingText = streamingReplyDisplay(full).
 * - ["reset", visibleText]: "assistant" event — App REPLACES full with the
 *   gateway's visible text and sets streamingText likewise.
 * - ["finish"]: done — App clears streaming + streamingText and saves
 *   content = unwrapReplyEnvelope(full).
 * The simulation steps in 16 ms ticks. It returns the rendered-content
 * length trace (what MessageRow would show each tick), the largest
 * single-tick growth of the RENDERED content, and the wall time until
 * the rendered content equals the saved content.
 */
function simulate(events, opts = {}) {
  const tickMs = TYPEWRITER_TICK_MS;
  const maxMs = opts.maxMs ?? 120000;
  let streaming = false;
  let streamingText = "";
  let displayed = "";
  let saved = "";
  let full = "";
  let animatedId = "placeholder";
  let lastAssistantId = "placeholder";
  const trace = []; // [timeMs, renderedLen, codeLen, displayedLen]
  let maxRenderedJump = 0;
  let prevRendered = 0;
  let t = 0;
  let ei = 0;
  // Turn start: placeholder added, streaming flips on, displayed resets.
  streaming = true;
  const renderLen = () => {
    const target = streaming ? streamingText : saved;
    const isTyping =
      (streaming || displayed.length < target.length) && lastAssistantId === animatedId;
    return isTyping ? displayed.length : saved.length;
  };
  const renderCodeLen = () => {
    const target = streaming ? streamingText : saved;
    const isTyping =
      (streaming || displayed.length < target.length) && lastAssistantId === animatedId;
    const content = isTyping ? displayed : saved;
    return codeLen(content);
  };
  while (t <= maxMs) {
    while (ei < events.length && events[ei][0] <= t) {
      const [, kind, payload] = events[ei++];
      if (kind === "delta") {
        full += payload;
        streamingText = streamingReplyDisplay(full);
      } else if (kind === "reset") {
        full = payload;
        streamingText = streamingReplyDisplay(full);
      } else if (kind === "finish") {
        streaming = false;
        streamingText = "";
        saved = unwrapReplyEnvelope(full);
      }
    }
    const target = streaming ? streamingText : saved;
    if (displayed.length < target.length) {
      const next = typewriterStep(target, displayed, streaming);
      if (next.length > displayed.length) displayed = next;
    }
    const r = renderLen();
    const rc = renderCodeLen();
    if (r - prevRendered > maxRenderedJump) maxRenderedJump = r - prevRendered;
    prevRendered = r;
    trace.push([t, r, rc, displayed.length]);
    if (!streaming && displayed.length >= target.length && r === saved.length) break;
    t += tickMs;
  }
  return { trace, maxRenderedJump, doneMs: t, saved };
}

check("stored 22:12 message is the exact 1452-char reply", () => {
  assert.equal(msg2212.length, 1452);
  assert.ok(msg2212.startsWith("Ini script kalkulator sederhananya, Angga."));
  assert.ok(msg2212.includes("```\n# Kalkulator sederhana"));
  assert.equal((msg2212.match(/```/g) || []).length, 2);
});

check("strip/unwrap are identity on the 22:12 content (no envelope, no tool fence)", () => {
  assert.equal(stripToolCallBlocks(msg2212), msg2212);
  assert.equal(unwrapReplyEnvelope(msg2212), msg2212);
  assert.equal(streamingReplyDisplay(msg2212), msg2212);
});

check("S1 single delta (local-tools path): no rendered jump, typing takes seconds", () => {
  const { trace, maxRenderedJump, doneMs, saved } = simulate([
    [0, "delta", msg2212],
    [200, "reset", msg2212],
    [400, "finish"],
  ]);
  assert.equal(saved, msg2212);
  assert.ok(maxRenderedJump <= 12, `max rendered jump ${maxRenderedJump} (want <= 12)`);
  assert.ok(doneMs >= 10000, `done in ${doneMs}ms (want >= 3000ms of visible typing)`);
  assert.ok(doneMs <= 35000, `done in ${doneMs}ms (want <= 15000ms)`);
  // The code part must grow progressively, not appear whole.
  const codeTrace = trace.map(([, , rc]) => rc);
  const codeMaxJump = Math.max(...codeTrace.map((v, i) => (i === 0 ? 0 : v - codeTrace[i - 1])));
  assert.ok(codeMaxJump <= 12, `max code growth per tick ${codeMaxJump} (want <= 12)`);
  const mid = trace.find(([, , rc]) => rc > 100 && rc < 1000);
  assert.ok(mid, "there is a mid-typing state with a partial code block");
  console.log(`    (S1: doneMs=${doneMs}, maxRenderedJump=${maxRenderedJump}, codeMaxJump=${codeMaxJump})`);
});

check("S2 chunked deltas (browser path): no rendered jump", () => {
  const chunk = 200;
  const events = [];
  for (let i = 0; i < msg2212.length; i += chunk) {
    events.push([i * 0.5, "delta", msg2212.slice(i, i + chunk)]);
  }
  events.push([1000, "reset", msg2212]);
  events.push([1200, "finish"]);
  const { maxRenderedJump, doneMs, saved } = simulate(events);
  assert.equal(saved, msg2212);
  assert.ok(maxRenderedJump <= 12, `max rendered jump ${maxRenderedJump} (want <= 12)`);
  assert.ok(doneMs >= 10000, `done in ${doneMs}ms`);
  console.log(`    (S2: doneMs=${doneMs}, maxRenderedJump=${maxRenderedJump})`);
});

check("S3 partial code still highlights (growing prefix of the 22:12 fence)", () => {
  const parts = splitParts(msg2212);
  assert.equal(parts[1], ""); // bare fence, no tag
  const code = parts[2];
  assert.ok(code.length > 1000);
  // First 50 chars are all comment ("# Kalkulator sederhana# Cara
  // jalanin: ...") — comment spans, no keywords yet.
  assert.ok(highlightCode(code.slice(0, 50), detectLang(code.slice(0, 50))).includes("tok-com"));
  for (const n of [200, 600, code.length]) {
    const prefix = code.slice(0, n);
    const html = highlightCode(prefix, detectLang(prefix));
    assert.ok(html.includes("tok-kw"), `prefix ${n} has keyword spans`);
  }
});

check("S4 code block header shows the detected language for the bare fence", () => {
  const parts = splitParts(msg2212);
  const effLang = parts[1] || detectLang(parts[2]);
  assert.equal(effLang, "python");
});

check("S5 RENDER path: the code block must grow progressively on screen, not appear whole", () => {
  // What the user actually sees each tick is NOT `displayed` raw — it is
  // MessageView's render of it, which runs stripToolCallBlocks first
  // (stripFences, assistant branch). Simulate the chase over the
  // single-delta target and measure the RENDERED code length per tick.
  const target = streamingReplyDisplay(msg2212);
  let displayed = "";
  let prevCode = 0;
  let maxCodeJump = 0;
  let sawPartialCode = false;
  let ticks = 0;
  while (displayed.length < target.length && ticks < 10000) {
    displayed = typewriterStep(target, displayed, true);
    const rendered = stripToolCallBlocks(displayed);
    const rc = codeLen(rendered);
    if (rc - prevCode > maxCodeJump) maxCodeJump = rc - prevCode;
    if (rc > 0 && rc < codeLen(msg2212)) sawPartialCode = true;
    prevCode = rc;
    ticks++;
  }
  assert.equal(displayed, target);
  assert.ok(maxCodeJump <= 12, `max rendered code growth per tick ${maxCodeJump} (want <= 12)`);
  assert.ok(sawPartialCode, "there is a mid-typing state with a partial code block on screen");
  console.log(`    (S5: ticks=${ticks}, maxCodeJump=${maxCodeJump}, sawPartialCode=${sawPartialCode})`);
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
