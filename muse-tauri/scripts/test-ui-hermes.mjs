// Node assertion tests for the Hermes UI round: the escape-first syntax
// highlighter (src/lib/highlight.ts) and the "Ran ..." card helpers +
// exit-code parser (src/lib/ranCard.ts). No test framework in the repo
// (same pattern as test-typewriter.mjs):
//   node scripts/test-ui-hermes.mjs
// It transpiles the REAL src/lib files with the repo's tsc into a temp
// dir and drives them directly, so the assertions exercise the shipped
// code, not a copy of it. Exits non-zero on any failure.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "ui-hermes-test-"));
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
      path.join(repo, "src", "lib", "highlight.ts"),
      path.join(repo, "src", "lib", "ranCard.ts"),
    ],
  }),
);
execFileSync(process.execPath, [path.join(repo, "node_modules", "typescript", "bin", "tsc"), "-p", tmpTsconfig], { stdio: "pipe" });
const { highlightCode, escapeHtml } = await import(pathToFileURL(path.join(tmp, "highlight.js")).href);
const { parseExitCode, ranTitle, ranCommandLine } = await import(pathToFileURL(path.join(tmp, "ranCard.js")).href);

let passed = 0;
let failed = 0;
function check(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  PASS ${name}`);
  } catch (e) {
    failed++;
    console.log(`  FAIL ${name}: ${e.message}`);
  }
}

console.log("highlighter (real src/lib/highlight.ts):");

check("escape-first: raw HTML in code never survives as markup", () => {
  const out = highlightCode('<script>alert("x")</script>', "python");
  assert.ok(!out.includes("<script>"), "raw <script> leaked");
  assert.ok(out.includes("&lt;script&gt;"), "escaped form missing");
  assert.ok(!out.includes('alert("x")'), "unescaped quote leaked");
});

check("python: keyword, function call, string, comment, number", () => {
  const out = highlightCode("def foo(x):  # hitung\n    return x + 42 + \"s\"", "python");
  assert.ok(out.includes('<span class="tok-kw">def</span>'), "def not keyword");
  assert.ok(out.includes('<span class="tok-fn">foo</span>'), "foo not function");
  assert.ok(out.includes('<span class="tok-com"># hitung</span>'), "comment missing");
  assert.ok(out.includes('<span class="tok-kw">return</span>'), "return not keyword");
  assert.ok(out.includes('<span class="tok-num">42</span>'), "42 not number");
  assert.ok(out.includes('<span class="tok-str">&quot;s&quot;</span>'), "string missing");
});

check("python: triple-quoted docstring is one string token", () => {
  const out = highlightCode('"""\nDocstring.\n"""\nx = 1', "python");
  assert.ok(out.includes("tok-str"), "docstring not a string");
  assert.ok(!out.includes("tok-kw"), "docstring words must not highlight as keywords");
});

check("js/ts: const/return keywords, template string, // comment", () => {
  const out = highlightCode("const s = `hi ${name}`; // halo\nreturn s;", "ts");
  assert.ok(out.includes('<span class="tok-kw">const</span>'));
  assert.ok(out.includes('<span class="tok-com">// halo</span>'));
  assert.ok(out.includes('<span class="tok-kw">return</span>'));
  assert.ok(out.includes("tok-str"), "template literal not a string");
});

check("rust: fn/let/mut keywords, string, line comment", () => {
  const out = highlightCode('fn main() {\n    let mut x = "a"; // halo\n}', "rust");
  assert.ok(out.includes('<span class="tok-kw">fn</span>'));
  assert.ok(out.includes('<span class="tok-kw">let</span>'));
  assert.ok(out.includes('<span class="tok-kw">mut</span>'));
  assert.ok(out.includes('<span class="tok-com">// halo</span>'));
});

check("bash: keyword + # comment + quoted string", () => {
  const out = highlightCode('if [ -f x ]; then\n  echo "ok" # selesai\nfi', "bash");
  assert.ok(out.includes('<span class="tok-kw">if</span>'));
  assert.ok(out.includes('<span class="tok-kw">then</span>'));
  assert.ok(out.includes('<span class="tok-com"># selesai</span>'));
});

check("json: keys get tok-key, values get tok-str/tok-num", () => {
  const out = highlightCode('{\n  "name": "muse",\n  "n": 3,\n  "ok": true\n}', "json");
  assert.ok(out.includes('<span class="tok-key">&quot;name&quot;</span>'), "key not tok-key");
  assert.ok(out.includes('<span class="tok-str">&quot;muse&quot;</span>'), "value not tok-str");
  assert.ok(out.includes('<span class="tok-num">3</span>'));
  assert.ok(out.includes('<span class="tok-kw">true</span>'));
});

check("sql: keywords match case-insensitively, -- comment", () => {
  const out = highlightCode("SELECT id FROM users WHERE id = 1 -- cari", "sql");
  assert.ok(out.includes('<span class="tok-kw">SELECT</span>'));
  assert.ok(out.includes('<span class="tok-kw">FROM</span>'));
  assert.ok(out.includes('<span class="tok-kw">WHERE</span>'));
  assert.ok(out.includes('<span class="tok-com">-- cari</span>'));
});

check("html: tag name, attribute, string, comment", () => {
  const out = highlightCode('<!-- hi -->\n<div class="box">teks</div>', "html");
  assert.ok(out.includes('<span class="tok-com">&lt;!-- hi --&gt;</span>'), "comment missing");
  assert.ok(out.includes('<span class="tok-tag">div</span>'), "tag missing");
  assert.ok(out.includes('<span class="tok-attr">class</span>'), "attr missing");
  assert.ok(out.includes('<span class="tok-str">&quot;box&quot;</span>'), "attr string missing");
  assert.ok(out.includes("teks"), "text content lost");
});

check("css: block comment, number, hex colour", () => {
  const out = highlightCode("/* tema */\n.a { color: #3867f5; margin: 8px; }", "css");
  assert.ok(out.includes('<span class="tok-com">/* tema */</span>'));
  assert.ok(out.includes('<span class="tok-num">#3867f5</span>'));
  assert.ok(out.includes('<span class="tok-num">8</span>'));
});

check("unknown language: plain escaped text, no crash", () => {
  const out = highlightCode("a < b && c > d", "brainfuck");
  assert.equal(out, escapeHtml("a < b && c > d"));
  assert.ok(!out.includes("tok-"));
});

console.log("ran cards + exit code (real src/lib/ranCard.ts):");

check("parseExitCode: 0 / 127 / -1 from metadata", () => {
  assert.equal(parseExitCode("exit_code=0\nstdout:\nok\n"), 0);
  assert.equal(parseExitCode("exit_code=127\nstderr:\nnot found\n"), 127);
  assert.equal(parseExitCode("exit_code=-1\nstdout:\n"), -1);
  assert.equal(parseExitCode("error: boom"), undefined);
  assert.equal(parseExitCode(undefined), undefined);
  assert.equal(parseExitCode(""), undefined);
});

check("parseExitCode: exit_code printed by the command itself is ignored", () => {
  // The real code lives after the stdout: boundary — metadata only.
  assert.equal(parseExitCode("stdout:\nexit_code=0\n"), undefined);
  assert.equal(parseExitCode("exit_code=0\nstdout:\nexit_code=1\n"), 0);
});

check("ranTitle: run_command shows the command, skill shows its name", () => {
  assert.equal(ranTitle("run_command", "run_command: npm test"), "Ran npm test");
  assert.equal(ranTitle("run_command", "npm run build"), "Ran npm run build");
  assert.equal(ranTitle("run_command"), "Ran command");
  assert.equal(ranTitle("skill", "local_seo"), "Ran skill: local_seo");
  assert.equal(ranTitle("read_file"), "Ran read_file");
});

check("ranCommandLine: command for run_command, path for file tools", () => {
  assert.equal(ranCommandLine("run_command", "run_command: cargo test", undefined), "cargo test");
  assert.equal(ranCommandLine("read_file", undefined, "src/App.tsx"), "src/App.tsx");
  assert.equal(ranCommandLine("search_files", "mencari foo", undefined), "mencari foo");
});

console.log(`\n${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
