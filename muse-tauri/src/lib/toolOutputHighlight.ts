// Presentation-only syntax colouring for native tool output, previews,
// terminal results, and file diffs. Reuses the escape-first VS Code
// tokenizer. No effect on tool execution, original output, or file writes.
import { detectLang, escapeHtml, highlightCode, langFromPath } from "./highlight.ts";

type Tone = "tok-key" | "tok-str" | "tok-num" | "tok-com" | "tok-type" | "tok-err";
const span = (tone: Tone, value: string) => '<span class="' + tone + '">' + escapeHtml(value) + "</span>";

function looksLikeCode(line: string): boolean {
  return /^\s*(?:(?:async\s+)?(?:def|class|fn|func|function|let|const|var|import|from|export|return|if|elif|else|for|while|try|except|catch|public|private|package|using|interface|struct|enum|SELECT|CREATE)\b|[@#][A-Za-z_]|\/\/|\/\*|\*|<\/?[a-z][\w-]*\b|\{|\}|\$\w+\s*=)/i.test(line)
    || /(?:\w+\s*\([^)]*\)\s*[:{]|=>|[=!<>]=|['"][^'"]+['"])/.test(line);
}

function codeHtml(code: string, lang: string): string {
  return highlightCode(code, lang || detectLang(code) || "javascript");
}

function metadataLine(line: string): string | null {
  const kv = /^(\s*)(File|Lines|content_sha256|sha256|exit_code|stdout|stderr|output|path|language|filename|error|warning|status|duration|source|cwd|line)(\s*[:=]\s*)(.*)$/i.exec(line);
  if (!kv) return null;
  const [, indent, key, separator, value] = kv;
  const bad = /^(?:stderr|error|warning)$/i.test(key);
  const numeric = /^-?\d+(?:\.\d+)?$/.test(value.trim());
  return escapeHtml(indent) + span(bad ? "tok-err" : "tok-key", key)
    + escapeHtml(separator) + (value ? span(bad ? "tok-err" : numeric ? "tok-num" : "tok-str", value) : "");
}

function receiptLine(line: string, tool: string, status: string): string | null {
  if (/^\s*(?:error\b|failed\b|traceback\b|exception\b|fatal\b|warning\b|modulenotfounderror\b)/i.test(line)
    || (status === "error" && !/^\s*$/.test(line))) return span("tok-err", line);
  if (/^(?:read_file|edit_file|write_file|run_command|run_script)/.test(tool)
    && /^\s*(?:edited|wrote|saved|created|applied|installed|completed|done|success)\b/i.test(line)) {
    const m = /^(\s*)([A-Za-z]+)(.*)$/.exec(line);
    return m ? escapeHtml(m[1]) + span("tok-type", m[2]) + span("tok-str", m[3]) : null;
  }
  return null;
}

/** Preserve every visible original character; only escaped HTML spans added.
 * The caller provides md-body scope so existing dark/light token colours work.
 */
export function highlightToolOutput(tool: string, text: string, path?: string, status = ""): string {
  const kind = tool.toLowerCase().trim();
  const read = kind === "read_file";
  const fileLang = langFromPath(path);
  const inferred = fileLang || detectLang(text);
  return text.split("\n").map((line) => {
    if (read) {
      const numbered = /^(\s*\d+:\s?)(.*)$/.exec(line);
      if (numbered) return span("tok-num", numbered[1]) + (numbered[2] ? codeHtml(numbered[2], fileLang) : "");
    }
    const meta = metadataLine(line);
    if (meta !== null) return meta;
    const receipt = receiptLine(line, kind, status);
    if (receipt !== null) return receipt;
    if (/^@@/.test(line)) return span("tok-key", line);
    if (/^diff --git /.test(line) || /^\+\+\+|^---/.test(line)) return span("tok-com", line);
    if (/^\+/.test(line) && (kind === "diff" || kind === "git_diff")) return span("tok-type", "+") + codeHtml(line.slice(1), fileLang);
    if (/^-/.test(line) && (kind === "diff" || kind === "git_diff")) return span("tok-err", "-") + codeHtml(line.slice(1), fileLang);
    if (looksLikeCode(line)) return codeHtml(line, inferred);
    if (/^\s*(?:[\w.\/\\-]+\.(?:py|rs|js|ts|tsx|jsx|json|css|html|sh|ps1)\b|\w+\s*[:=]\s*['"\d])/.test(line)) {
      return codeHtml(line, inferred);
    }
    return escapeHtml(line);
  }).join("\n");
}

/** A real unified diff is coloured without modifying its underlying lines. */
export function highlightCodeDiff(text: string, path?: string): string {
  const language = langFromPath(path);
  return text.split("\n").map((line) => {
    if (/^@@/.test(line) || /^diff --git /.test(line)) return span("tok-key", line);
    if (/^\+\+\+|^---/.test(line)) return span("tok-com", line);
    if (/^\+/.test(line)) return span("tok-type", "+") + codeHtml(line.slice(1), language);
    if (/^-/.test(line)) return span("tok-err", "-") + codeHtml(line.slice(1), language);
    if (/^ /.test(line)) return escapeHtml(" ") + codeHtml(line.slice(1), language);
    return escapeHtml(line);
  }).join("\n");
}
