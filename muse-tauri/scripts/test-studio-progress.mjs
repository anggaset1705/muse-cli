// Regression tests: live Studio progress must show real gateway/task state.
// No browser login, model requests, or render quota are used here.
import fs from "node:fs";
import vm from "node:vm";
import path from "node:path";
import ts from "typescript";

const root = path.resolve(import.meta.dirname, "..");
const source = fs.readFileSync(path.join(root, "src/lib/studioProgress.ts"), "utf8");
const js = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const sandbox = { exports: {}, module: { exports: {} } };
sandbox.module.exports = sandbox.exports;
vm.runInNewContext(js, sandbox, { timeout: 1000 });
const { studioProjectIdFromAction, studioProgressTerminal, studioProgressPercent } = sandbox.exports;

function assert(value, desc) {
  if (!value) throw new Error("FAILED: " + desc);
  console.log("PASS: " + desc);
}
const render = {
  tool: "studio_render_project", status: "done",
  result: "studio_render_started: muse-video-c71f6273f1\nVideo scenes render sequentially...",
};
assert(studioProjectIdFromAction(render) === "muse-video-c71f6273f1", "real video queue receipt activates monitor");
assert(studioProjectIdFromAction({tool:"studio_render_project", status:"done", result:"error: failed"}) === null, "tool error cannot activate monitor");
assert(studioProjectIdFromAction({tool:"run_command",status:"done",result:"studio_render_started: dummy"}) === null, "ordinary coding card never becomes Studio monitor");
assert(studioProjectIdFromAction({
  tool:"studio_render_image",status:"done",
  result:"studio_image_queued: img_123\nproject: muse-image-example\nscene: 1",
}) === "muse-image-example","real image receipt also supported");
assert(studioProjectIdFromAction({
  tool:"studio_render_project",status:"done",result:"studio_render_started: ../../escape"
}) === null, "reject path traversal in project identifier");

const running = { state:"rendering", progress:63 };
const saving = { state:"saving", progress:100 };
const done = { state:"complete", progress:100 };
const failed = { state:"failed", progress:30 };
assert(studioProgressPercent(running) === 63, "use real provider progress");
assert(studioProgressPercent(saving) === 99, "provider 100% is not full project completion");
assert(studioProgressPercent(done) === 100, "only finished project may show 100%");
assert(!studioProgressTerminal(running) && !studioProgressTerminal(saving), "keep polling during render and save");
assert(studioProgressTerminal(done) && studioProgressTerminal(failed), "stop polling on done or failed");

const component = fs.readFileSync(path.join(root, "src/components/StudioProgressCard.tsx"), "utf8");
const card = fs.readFileSync(path.join(root, "src/components/ChatPanel.tsx"), "utf8");
const app = fs.readFileSync(path.join(root, "src/App.tsx"), "utf8");
const api = fs.readFileSync(path.join(root, "src/lib/gateway.ts"), "utf8");
assert(component.includes("setTimeout(check, 3500)"), "poll real status every 3.5 seconds, not fake percentage timer");
assert(component.includes("controller.abort()"), "stop HTTP request and timer when changing conversations");
assert(component.includes("studioProgressTerminal(snapshot)"), "polling stops automatically on terminal status");
assert(component.includes("final_video_path"), "finished MP4 path shown from server");
assert(card.includes("studioProjectIdFromAction(a)") && card.includes("<StudioProgressCard"),"progress embedded under native Ran card");
assert(app.includes("agentSessionsRef.current[active.id]"),"only active conversation session used");
assert(api.includes("/studio/") && api.includes("encodeURIComponent(projectId)"),"authenticated workspace-specific status endpoint");
console.log("STUDIO_LIVE_PROGRESS_TESTS_ALL_PASS");
