// Minimal safe markdown renderer: code blocks, inline code, bold,
// italic, links, headings, lists. HTML in user/AI text is escaped first.
import { useMemo } from "react";

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function renderInline(s: string): string {
  let out = escapeHtml(s);
  out = out.replace(/`([^`\n]+)`/g, "<code class=\"md-code\">$1</code>");
  out = out.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  out = out.replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<em>$2</em>");
  out = out.replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer" class="md-link">$1</a>');
  return out;
}

function renderMarkdown(content: string): string {
  const blocks: string[] = [];
  // Split out fenced code blocks first.
  const parts = content.split(/```(\w*)\n?/);
  // parts: [text, lang, code, text, lang, code, ...]
  for (let i = 0; i < parts.length; i++) {
    if (i % 3 === 2) {
      const code = escapeHtml(parts[i]);
      const lang = parts[i - 1] || "";
      blocks.push(
        `<pre class="md-pre"><div class="md-pre-head">${escapeHtml(lang) || "code"}</div><code>${code}</code></pre>`,
      );
    } else if (i % 3 === 0) {
      const text = parts[i];
      const lines = text.split("\n");
      let html = "";
      let inList = false;
      for (const line of lines) {
        const trimmed = line.trim();
        if (trimmed.startsWith("- ") || trimmed.startsWith("* ")) {
          if (!inList) {
            html += "<ul class=\"md-ul\">";
            inList = true;
          }
          html += `<li>${renderInline(trimmed.slice(2))}</li>`;
          continue;
        }
        if (inList) {
          html += "</ul>";
          inList = false;
        }
        if (trimmed.startsWith("### ")) {
          html += `<h3 class="md-h3">${renderInline(trimmed.slice(4))}</h3>`;
        } else if (trimmed.startsWith("## ")) {
          html += `<h2 class="md-h2">${renderInline(trimmed.slice(3))}</h2>`;
        } else if (trimmed.startsWith("# ")) {
          html += `<h2 class="md-h2">${renderInline(trimmed.slice(2))}</h2>`;
        } else if (trimmed === "") {
          // paragraph break — nothing to emit
        } else {
          html += `<p class="md-p">${renderInline(trimmed)}</p>`;
        }
      }
      if (inList) html += "</ul>";
      blocks.push(html);
    }
  }
  return blocks.join("");
}

export function MessageView({ content }: { content: string }) {
  const html = useMemo(() => renderMarkdown(content), [content]);
  return <div className="md-body" dangerouslySetInnerHTML={{ __html: html }} />;
}
