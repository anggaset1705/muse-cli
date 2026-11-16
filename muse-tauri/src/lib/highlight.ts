// Tiny syntax highlighter, VS Code Dark+ palette — no new dependency.
// Escape-first: every emitted token is HTML-escaped BEFORE it is wrapped
// in a <span>, so code can never inject markup into MessageView's
// dangerouslySetInnerHTML. Only applied to complete fenced blocks (the
// typewriter holds unclosed fences, so a half block never reaches here).
//
// Supported: python, javascript/typescript, rust, bash/sh, json, sql,
// html, css, yaml. Unknown languages fall back to plain escaped text.

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

type LangCfg = {
  keywords: Set<string>;
  /** Case-insensitive keyword match (SQL). */
  keywordsCi?: boolean;
  lineComment?: string[];
  blockComment?: boolean;
  /** String delimiters; python adds triple quotes. */
  strings: string[];
  tripleStrings?: boolean;
};

const PY_KW = [
  "def", "class", "return", "if", "elif", "else", "for", "while", "import",
  "from", "as", "try", "except", "finally", "with", "lambda", "pass",
  "break", "continue", "and", "or", "not", "in", "is", "None", "True",
  "False", "raise", "yield", "async", "await", "global", "nonlocal",
  "del", "assert",
];
const JS_KW = [
  "const", "let", "var", "function", "return", "if", "else", "for", "while",
  "class", "extends", "new", "import", "from", "export", "default", "try",
  "catch", "finally", "throw", "async", "await", "typeof", "instanceof",
  "in", "of", "switch", "case", "break", "continue", "this", "super",
  "null", "undefined", "true", "false", "do", "void", "delete", "static",
  "get", "set",
];
const RS_KW = [
  "fn", "let", "mut", "struct", "enum", "impl", "trait", "pub", "use",
  "mod", "return", "if", "else", "for", "while", "loop", "match", "break",
  "continue", "const", "static", "ref", "move", "in", "as", "where",
  "unsafe", "extern", "crate", "self", "Self", "super", "true", "false",
  "type", "dyn", "async", "await",
];
const SH_KW = [
  "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case",
  "esac", "function", "in", "echo", "cd", "export", "local", "return",
  "break", "continue", "set", "source", "alias", "select", "until",
];
const SQL_KW = [
  "select", "from", "where", "insert", "into", "values", "update", "set",
  "delete", "create", "table", "drop", "alter", "join", "left", "right",
  "inner", "outer", "on", "as", "and", "or", "not", "null", "primary",
  "key", "distinct", "order", "by", "group", "having", "limit", "offset",
  "index", "unique", "default", "exists", "between", "like", "is", "in",
  "union", "all", "count", "sum", "avg", "max", "min",
];
const CSS_KW = ["media", "import", "keyframes", "font-face", "supports"];

function cfg(lang: string): LangCfg | null {
  const l = lang.toLowerCase();
  switch (l) {
    case "py":
    case "python":
      return { keywords: new Set(PY_KW), lineComment: ["#"], strings: ["'", '"'], tripleStrings: true };
    case "js":
    case "javascript":
    case "ts":
    case "tsx":
    case "jsx":
    case "typescript":
      return { keywords: new Set(JS_KW), lineComment: ["//"], blockComment: true, strings: ["'", '"', "`"] };
    case "rs":
    case "rust":
      return { keywords: new Set(RS_KW), lineComment: ["//"], blockComment: true, strings: ['"', "'"] };
    case "sh":
    case "bash":
    case "shell":
      return { keywords: new Set(SH_KW), lineComment: ["#"], strings: ["'", '"', "`"] };
    case "json":
      return { keywords: new Set(["true", "false", "null"]), strings: ['"'] };
    case "sql":
      return { keywords: new Set(SQL_KW), keywordsCi: true, lineComment: ["--"], blockComment: true, strings: ["'", '"', "`"] };
    case "css":
      return { keywords: new Set(CSS_KW), lineComment: [], blockComment: true, strings: ["'", '"'] };
    case "yaml":
    case "yml":
      return { keywords: new Set(["true", "false", "null"]), lineComment: ["#"], strings: ["'", '"'] };
    default:
      return null;
  }
}

function span(cls: string, text: string): string {
  return `<span class="${cls}">${escapeHtml(text)}</span>`;
}

/** Highlight one complete code block. Returns escaped HTML with spans. */
export function highlightCode(code: string, lang: string): string {
  const l = lang.toLowerCase();
  if (l === "html" || l === "xml") return highlightHtml(code);
  const c = cfg(l);
  if (!c) return escapeHtml(code);

  let out = "";
  let i = 0;
  const n = code.length;
  while (i < n) {
    const rest = code.slice(i);

    // Block comment /* ... */
    if (c.blockComment && rest.startsWith("/*")) {
      const end = code.indexOf("*/", i + 2);
      const stop = end === -1 ? n : end + 2;
      out += span("tok-com", code.slice(i, stop));
      i = stop;
      continue;
    }
    // Line comment
    let matchedComment = false;
    for (const marker of c.lineComment ?? []) {
      if (rest.startsWith(marker)) {
        // SQL "--" needs a space/control after it; "#" and "//" do not.
        if (marker === "--" && i + 2 < n && !/[\s]/.test(code[i + 2])) continue;
        const nl = code.indexOf("\n", i);
        const stop = nl === -1 ? n : nl;
        out += span("tok-com", code.slice(i, stop));
        i = stop;
        matchedComment = true;
        break;
      }
    }
    if (matchedComment) continue;

    // Triple-quoted string (python docstrings)
    if (c.tripleStrings && (rest.startsWith('"""') || rest.startsWith("'''"))) {
      const q = rest.slice(0, 3);
      const end = code.indexOf(q, i + 3);
      const stop = end === -1 ? n : end + 3;
      out += span("tok-str", code.slice(i, stop));
      i = stop;
      continue;
    }
    // String
    if (c.strings.includes(rest[0])) {
      const q = rest[0];
      let j = i + 1;
      while (j < n) {
        if (code[j] === "\\") {
          j += 2;
          continue;
        }
        if (code[j] === q) {
          j++;
          break;
        }
        // A newline ends a single-quoted string (except backtick templates).
        if (code[j] === "\n" && q !== "`") break;
        j++;
      }
      const tok = code.slice(i, j);
      // JSON: a string followed by ":" is an object key.
      if (l === "json") {
        let k = j;
        while (k < n && /\s/.test(code[k])) k++;
        out += code[k] === ":" ? span("tok-key", tok) : span("tok-str", tok);
      } else {
        out += span("tok-str", tok);
      }
      i = j;
      continue;
    }
    // Number (incl. 0x hex, floats, 1_000 separators)
    const num = /^(?:0[xX][0-9a-fA-F_]+|\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(rest);
    if (num) {
      out += span("tok-num", num[0]);
      i += num[0].length;
      continue;
    }
    // CSS hex colour #fff / #ffffff
    if (l === "css" && rest[0] === "#") {
      const hex = /^#[0-9a-fA-F]{3,8}\b/.exec(rest);
      if (hex) {
        out += span("tok-num", hex[0]);
        i += hex[0].length;
        continue;
      }
    }
    // Identifier / keyword / function call
    const word = /^[A-Za-z_$][\w$-]*/.exec(rest);
    if (word) {
      const w = word[0];
      const key = c.keywordsCi ? w.toLowerCase() : w;
      if (c.keywords.has(key)) {
        out += span("tok-kw", w);
      } else {
        // Function call: identifier followed by "(" (ignoring whitespace).
        let k = i + w.length;
        while (k < n && /\s/.test(code[k])) k++;
        out += code[k] === "(" ? span("tok-fn", w) : escapeHtml(w);
      }
      i += w.length;
      continue;
    }
    // Anything else: one escaped char.
    out += escapeHtml(code[i]);
    i++;
  }
  return out;
}

/** Minimal HTML highlighter: comments, tag names, attributes, strings. */
function highlightHtml(code: string): string {
  let out = "";
  let i = 0;
  const n = code.length;
  while (i < n) {
    // Comment <!-- ... -->
    if (code.startsWith("<!--", i)) {
      const end = code.indexOf("-->", i + 4);
      const stop = end === -1 ? n : end + 3;
      out += span("tok-com", code.slice(i, stop));
      i = stop;
      continue;
    }
    // Tag <...>
    if (code[i] === "<") {
      const end = code.indexOf(">", i + 1);
      const stop = end === -1 ? n : end + 1;
      out += highlightTag(code.slice(i, stop));
      i = stop;
      continue;
    }
    // Text between tags: escape, up to the next "<".
    const next = code.indexOf("<", i);
    const stop = next === -1 ? n : next;
    out += escapeHtml(code.slice(i, stop));
    i = stop;
  }
  return out;
}

function highlightTag(tag: string): string {
  // <, </, tag name, attrs, strings, >, />
  const m = /^<(\/?)([A-Za-z][\w-]*)?/.exec(tag);
  if (!m) return escapeHtml(tag);
  let out = span("tok-punc", "<" + (m[1] ?? ""));
  let pos = m[0].length;
  if (m[2]) out += span("tok-tag", m[2]);
  const rest = tag.slice(pos);
  // Attribute names (word before =), strings, and the rest as punctuation.
  const attrRe = /([A-Za-z_:][\w:.-]*)(?=\s*=)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')|([^"'\s]+)/g;
  let mm: RegExpExecArray | null;
  while ((mm = attrRe.exec(rest)) !== null) {
    if (mm[1] !== undefined) out += span("tok-attr", mm[1]);
    else if (mm[2] !== undefined) out += span("tok-str", mm[2]);
    else out += span("tok-punc", mm[3]);
    // Whitespace between tokens is preserved by the regex gap: re-add it.
    const gapEnd = attrRe.lastIndex;
    const next = rest.slice(gapEnd).match(/^\s+/);
    if (next) {
      out += escapeHtml(next[0]);
      attrRe.lastIndex += next[0].length;
    }
  }
  return out;
}

/** Highlight language for a workspace path, from its extension. Unknown
 * extensions (or none) return "" so highlightCode falls back to plain
 * escaped text. Used by the live-typed code view inside a "Ran ..." card
 * (13 Nov 2026): the card only knows the target path, not a language. */
export function langFromPath(path?: string): string {
  if (!path) return "";
  const base = path.split(/[\\/]/).filter(Boolean).pop() ?? "";
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return "";
  switch (base.slice(dot + 1).toLowerCase()) {
    case "py":
      return "python";
    case "js":
    case "jsx":
      return "javascript";
    case "ts":
    case "tsx":
      return "typescript";
    case "rs":
      return "rust";
    case "sh":
    case "bash":
      return "bash";
    case "json":
      return "json";
    case "sql":
      return "sql";
    case "html":
    case "htm":
      return "html";
    case "css":
      return "css";
    case "yaml":
    case "yml":
      return "yaml";
    default:
      return "";
  }
}

/** Guess a language from code content, for fenced blocks with no language
 * tag. Bree's 18:31 live test: the agent emitted a bare ``` fence (no
 * "python" tag), so highlightCode fell back to plain escaped text and the
 * code showed in a single colour. Rules are high-precision markers,
 * checked in order; anything unrecognised returns "" (plain text, as
 * before). */
export function detectLang(code: string): string {
  const t = code.trim();
  if (!t) return "";
  // JSON: parses as a whole object/array.
  if (t.startsWith("{") || t.startsWith("[")) {
    try {
      JSON.parse(t);
      return "json";
    } catch {
      /* not JSON — fall through */
    }
  }
  // HTML: a tag plus a closing tag (or a doctype).
  if (/<!doctype html/i.test(t)) return "html";
  if (/<[a-z][\w-]*(\s[^>]*)?>/i.test(t) && /<\/[a-z][\w-]*>/i.test(t)) return "html";
  // Python: def / __main__ / elif / from-import / import-as / print("...").
  if (
    /\bdef\s+\w+\s*\(/.test(t) ||
    /if\s+__name__\s*==/.test(t) ||
    /^\s*elif\s/m.test(t) ||
    /\bfrom\s+[\w.]+\s+import\s/.test(t) ||
    /^\s*import\s+[\w.]+\s+as\s/m.test(t) ||
    /\bprint\s*\(["']/.test(t) ||
    // A `#` comment line that names the interpreter ("python ...") is
    // a Python script still streaming in (its `def` lines have not
    // arrived yet) — colour the comment from the first character
    // instead of leaving the earliest typed prefix plain.
    /^#.*\bpython\b/im.test(t)
  ) {
    return "python";
  }
  // Rust: fn / println! / let mut.
  if (/\bfn\s+\w+\s*\(/.test(t) || /println!\s*\(/.test(t) || /\blet\s+mut\s/.test(t)) return "rust";
  // JavaScript/TypeScript: function / console.log / => / require( / interface.
  if (
    /\bfunction\s+\w+\s*\(/.test(t) ||
    /console\.(log|warn|error)\s*\(/.test(t) ||
    /=>/.test(t) ||
    /\b(const|let|var)\s+[\w$]+\s*=\s*require\s*\(/.test(t) ||
    /\binterface\s+\w+\s*\{/.test(t)
  ) {
    return "javascript";
  }
  // Bash: shebang, echo with a quote/$ argument, or `if ...; then`.
  if (/^#!.*\b(bash|sh)\b/m.test(t) || /^\s*echo\s+["'`$]/m.test(t) || /\bif\s+[^;\n]+;\s*then\b/.test(t)) return "bash";
  // SQL: a statement keyword at a line start.
  if (/^\s*(SELECT|INSERT\s+INTO|UPDATE\s+\w+\s+SET|DELETE\s+FROM|CREATE\s+TABLE|DROP\s+TABLE|ALTER\s+TABLE)\b/im.test(t)) {
    return "sql";
  }
  // CSS: a selector followed by `{ prop: value; }`.
  if (/[\w.#:[\]-]+\s*\{[^}]*[\w-]+\s*:\s*[^;{}]+;[^}]*\}/.test(t)) return "css";
  // YAML: at least two `key: value` lines and no braces/brackets.
  const yamlLines = t.split("\n").filter((l) => /^[\w.-]+\s*:\s+\S/.test(l));
  if (yamlLines.length >= 2 && !/[{}[\]]/.test(t)) return "yaml";
  return "";
}
