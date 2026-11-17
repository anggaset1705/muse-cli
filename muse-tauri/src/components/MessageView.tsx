// Minimal safe markdown renderer: code blocks, inline code, bold,
// italic, links, headings, lists. HTML in user/AI text is escaped first.
//
// Code blocks are React components (not a string): the header carries a
// Copy button (Hermes pattern) and the body is highlighted with the
// escape-first tokenizer in @/lib/highlight. Highlighting only ever
// runs on a complete fenced block — the typewriter holds an unclosed
// fence at its opener, so a half block never reaches this renderer.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { highlightCode, detectLang } from "@/lib/highlight";
import { stripToolCallBlocks } from "@/lib/replyEnvelope";
import { reconcileScrollFollow } from "@/lib/scrollFollow";
import { isLiveCodeGrowth } from "@/lib/codeScrollFollow";
import { t, type Lang } from "@/lib/i18n";

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

/** One fenced code block: language label + Copy button in the header,
 * VS Code Dark+ highlighting in the body. */
function CodeBlock({
  lang, code, uiLang, followLiveCode = false,
}: {
  lang: string;
  code: string;
  uiLang: Lang;
  /** Only a currently typing reply should follow the newest code line. */
  followLiveCode?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const codePreRef = useRef<HTMLPreElement>(null);
  const followCodeRef = useRef(true);
  const manualCodeScrollRef = useRef(false);
  const previousCodeRef = useRef<string | null>(null);
  const codeGrowingRef = useRef(false);
  // Markdown assistant code can KEEP typing after the gateway finishes its
  // turn and streaming=false. Use actual growing content as evidence rather
  // than relying solely on the current streaming flag.
  // Crucially, this scroll state belongs to the CODE CARD, not the chat.
  useLayoutEffect(() => {
    const previous = previousCodeRef.current;
    codeGrowingRef.current = isLiveCodeGrowth(previous, code, followLiveCode);
    previousCodeRef.current = code;
    if (!codeGrowingRef.current || !followCodeRef.current) return;
    const el = codePreRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    const frame = window.requestAnimationFrame(() => {
      if (followCodeRef.current && codeGrowingRef.current) {
        el.scrollTop = el.scrollHeight;
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [code, followLiveCode]);
  // The highlighted HTML and wrapped lines can finish layout *after* React
  // commits. Observe THIS inner code area so the caret remains visible even
  // while the user scrolls the OUTER conversation up to read older messages.
  useEffect(() => {
    const el = codePreRef.current;
    if (!el) return;
    let frame = 0;
    const requestFollow = () => {
      if (!codeGrowingRef.current || !followCodeRef.current || frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        if (codeGrowingRef.current && followCodeRef.current) {
          el.scrollTop = el.scrollHeight;
        }
      });
    };
    const observer = new MutationObserver(requestFollow);
    observer.observe(el, { childList: true, subtree: true, characterData: true });
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(requestFollow);
    if (resize) resize.observe(el.querySelector("code") ?? el);
    return () => {
      observer.disconnect();
      resize?.disconnect();
      if (frame) window.cancelAnimationFrame(frame);
    };
  }, []);
  const onCodeScroll = () => {
    const el = codePreRef.current;
    if (!el) return;
    followCodeRef.current = reconcileScrollFollow(
      followCodeRef.current, manualCodeScrollRef.current, el, 32,
    );
    manualCodeScrollRef.current = false;
  };
  // A bare fence (no language tag — the 18:31 agent reply) would fall
  // back to plain text; guess the language from the content instead.
  const effLang = lang || detectLang(code);
  const html = useMemo(() => highlightCode(code, effLang), [code, effLang]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };
  return (
    <div className="md-code-shell">
      <div className="md-pre-head">
        <span>{effLang || "code"}</span>
        <button
          type="button"
          onClick={copy}
          className="md-copy-btn"
          title={t(uiLang, "copyCode")}
          aria-label={t(uiLang, "copyCode")}
        >
          {copied ? <Check className="h-3.5 w-3.5 text-emerald-400" /> : <Copy className="h-3.5 w-3.5" />}
          {copied ? t(uiLang, "copied") : ""}
        </button>
      </div>
      <pre
        ref={codePreRef}
        className="md-pre md-code-scroll"
        style={{ overflowAnchor: "none", overscrollBehavior: "contain" }}
        tabIndex={0}
        onScroll={onCodeScroll}
        onWheel={(e) => {
          e.stopPropagation(); // Do not pause the outer chat follow.
          manualCodeScrollRef.current = true;
          if (e.deltaY < 0) followCodeRef.current = false;
        }}
        onPointerDown={(e) => {
          e.stopPropagation();
          if (e.clientX >= e.currentTarget.getBoundingClientRect().right - 18) {
            manualCodeScrollRef.current = true;
            followCodeRef.current = false;
          }
        }}
        onTouchMove={(e) => {
          e.stopPropagation();
          manualCodeScrollRef.current = true;
          followCodeRef.current = false;
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (["ArrowUp", "PageUp", "Home"].includes(e.key)) followCodeRef.current = false;
          if (["ArrowUp", "PageUp", "Home", "ArrowDown", "PageDown", "End"].includes(e.key)) {
            manualCodeScrollRef.current = true;
          }
        }}
      >
        <code dangerouslySetInnerHTML={{ __html: html }} />
      </pre>
    </div>
  );
}

function renderTextBlock(text: string): string {
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
  return html;
}

export function MessageView({
  content,
  lang,
  stripFences,
  typingTarget,
}: {
  content: string;
  lang?: Lang;
  /** Assistant messages only: drop tool-call fence shells (label + empty
   *  block) at render time too, so an artifact already stored before the
   *  save-time strip was fixed (e.g. Bree's 20:53 turn) no longer shows.
   *  User messages keep stripFences off — their text is never stripped. */
  stripFences?: boolean;
  /** Confirmed full code target only for assistant typewriter. */
  typingTarget?: string;
}) {
  const uiLang: Lang = lang ?? "en";
  const nodes = useMemo(() => {
    // Split out fenced code blocks first.
    const display = stripFences ? stripToolCallBlocks(content, typingTarget) : content;
    const parts = display.split(/```(\w*)\n?/);
    // parts: [text, lang, code, text, lang, code, ...]
    return parts.map((part, i) => {
      if (i % 3 === 2) {
        return <CodeBlock key={i} lang={parts[i - 1] || ""} code={part} uiLang={uiLang} followLiveCode={typingTarget !== undefined} />;
      }
      if (i % 3 === 0) {
        const html = renderTextBlock(part);
        return html ? (
          // eslint-disable-next-line react/no-danger
          <div key={i} dangerouslySetInnerHTML={{ __html: html }} />
        ) : null;
      }
      return null; // i % 3 === 1 is the language tag, consumed by CodeBlock
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [content, uiLang, stripFences, typingTarget]);
  return <div className="md-body">{nodes}</div>;
}
