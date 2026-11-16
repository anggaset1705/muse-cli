/** True when a parsed JSON value is a planner/tool-call payload: an
 * object with an `actions` or `tool_calls` array (the shapes
 * parser::parse accepts). A user-asked JSON block with other keys is
 * prose and is never stripped. */
function isToolCallPayload(value: unknown): boolean {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const rec = value as Record<string, unknown>;
  return Array.isArray(rec.actions) || Array.isArray(rec.tool_calls);
}

/**
 * Remove planner/tool-call JSON from display text (13 Nov 2026 — Bree
 * saw raw JSON flash in the chat while a tool-call block streamed in:
 * the driver forwards the raw model reply as deltas, and the old code
 * only hid status/summary envelopes, never tool-call blocks).
 *
 * - Fenced ```json / ```tool / ``` (empty-language) blocks whose body
 *   parses as a tool-call payload are removed outright.
 * - An UNCLOSED ```json / ```tool fence is hidden from its opener on —
 *   the block is still streaming in (the typewriter already holds
 *   unclosed fences the same way, so nothing that was visible is lost).
 * - A bare whole-text JSON object with an actions / tool_calls array is
 *   removed too (the single-delta local-tools path can deliver the
 *   whole reply as one such object).
 * - Anything else (plain text, a legit JSON code block the user asked
 *   for) passes through untouched.
 */
/**
 * A stray fence-language label ("json"/"tool") rendered as its own line
 * directly before a dropped fence (13 Nov 2026 — Bree's 20:53 live test:
 * the reply came out as the word "json" + an empty code block). The web
 * UI renders a code block's language header as text, so the reply text
 * can carry a "json" line in front of the tool-call fence; the gateway
 * strips such a line per step, but the frontend sees the concatenated
 * text. Drop the label line (plus the blank lines between it and the
 * fence) together with the fence. Only the tail of the gap is touched —
 * prose before the label is kept.
 */
function dropLabelTail(gap: string): string {
  return gap.replace(/(?:^|\n)[ \t]*(?:json|tool)[ \t\r]*(?:\r?\n[ \t\r]*)*$/i, "");
}

/** During typing only, a complete target proves that an unfinished
 * rendered prefix belongs to a legitimate (non-tool) code fence.
 * Default protocol filtering remains conservative and unchanged. */
function isConfirmedLegitimateFence(
  prefix: string,
  openingIndex: number,
  confirmedTarget?: string,
): boolean {
  if (!confirmedTarget || !confirmedTarget.startsWith(prefix)) return false;
  const nl = confirmedTarget.indexOf("\n", openingIndex + 3);
  if (nl === -1) return false;
  const closer = "\n" + String.fromCharCode(96).repeat(3);
  const end = confirmedTarget.indexOf(closer, nl);
  if (end === -1) return false;
  const body = confirmedTarget.slice(nl + 1, end).trim();
  if (!body) return false;
  try {
    if (isToolCallPayload(JSON.parse(body))) return false;
  } catch {
    // Normal source code is not a parsed tool-call JSON object.
  }
  return true;
}
export function stripToolCallBlocks(text: string, confirmedTarget?: string): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const open = text.indexOf("```", i);
    if (open === -1) {
      out += text.slice(i);
      break;
    }
    // A fence opener must start a line (or the text); ``` mid-line is
    // inline code, not a fence.
    if (open > 0 && text[open - 1] !== "\n") {
      out += text.slice(i, open + 3);
      i = open + 3;
      continue;
    }
    const nl = text.indexOf("\n", open + 3);
    const tag = (nl === -1 ? text.slice(open + 3) : text.slice(open + 3, nl)).trim().toLowerCase();
    const isProto = tag === "json" || tag === "tool" || tag === "";
    // Search the closer from the tag line's own newline: an empty body
    // ("```json\n```") closes on the very next line.
    const close = nl === -1 ? -1 : text.indexOf("\n```", nl);
    if (!isProto) {
      // Not a protocol fence: keep it whole — unless it is empty.
      // A closed fence with a whitespace-only body renders as an empty
      // code block (Bree's 20:53 live test); it carries no content, so
      // drop it like a tool-call fence. An unclosed fence is still
      // streaming in, so it is kept whole as before.
      if (close === -1) {
        out += text.slice(i);
        break;
      }
      if (text.slice(nl + 1, close).trim() === "") {
        out += dropLabelTail(text.slice(i, open));
        i = close + 4;
        if (text[i] === "\r" && text[i + 1] === "\n") i += 2;
        else if (text[i] === "\n") i += 1;
        continue;
      }
      out += text.slice(i, close + 4);
      i = close + 4;
      continue;
    }
    if (close === -1) {
      if (isConfirmedLegitimateFence(text, open, confirmedTarget)) {
        out += text.slice(i);
        break;
      }
      // Unclosed protocol fence. A json/tool fence is a protocol block
      // still streaming in — hide it. A BARE fence (tag "") is usually a
      // code block still being typed: hiding it made the whole code
      // block appear at once in Bree's 22:12 live test (the render strip
      // hid the partial fence until it closed, then the full block
      // popped in one tick). Keep a bare fence whole so it types live,
      // UNLESS its partial body already looks like a tool-call payload
      // (same "tool_calls"/"actions" marker rule as the bare-object
      // tail below) — that still hides a streaming tool call.
      if (tag === "") {
        const partialBody = nl === -1 ? "" : text.slice(nl + 1);
        if (!/^\s*\{/.test(partialBody) || !/"tool_calls"\s*:|"actions"\s*:/.test(partialBody)) {
          out += text.slice(i);
          break;
        }
      }
      // Unclosed json/tool fence (or a bare fence that already looks
      // like a tool call): hide from the opener onward (and any stray
      // label line directly before it).
      out += dropLabelTail(text.slice(i, open));
      break;
    }
    const body = text.slice(nl + 1, close);
    // An empty protocol fence is the shell left behind after a tool
    // call (Bree's 20:53 live test: "json" + an empty code block) — it
    // carries no content, so drop it together with any stray label
    // line directly before it.
    if (body.trim() === "") {
      out += dropLabelTail(text.slice(i, open));
      i = close + 4;
      if (text[i] === "\r" && text[i + 1] === "\n") i += 2;
      else if (text[i] === "\n") i += 1;
      continue;
    }
    let isCall = false;
    try {
      isCall = isToolCallPayload(JSON.parse(body.trim()));
    } catch {
      isCall = false;
    }
    if (isCall) {
      out += dropLabelTail(text.slice(i, open)); // drop the fence entirely
      i = close + 4;
      // The fence occupied its own lines: drop the line break after it
      // too, so the following text does not start with a blank line.
      if (text[i] === "\r" && text[i + 1] === "\n") i += 2;
      else if (text[i] === "\n") i += 1;
    } else {
      out += text.slice(i, close + 4);
      i = close + 4;
    }
  }
  // The fence may already have been removed upstream (the gateway
  // strips tool-call fences per step), leaving only its stray label
  // line behind. A reply consisting solely of "json"/"tool" is not
  // prose — drop it.
  if (/^(?:json|tool)$/i.test(out.trim())) return "";
  const trimmed = out.trim();
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try {
      if (isToolCallPayload(JSON.parse(trimmed))) return "";
    } catch {
      // Still streaming in: hide it while it already looks like a
      // tool-call payload (a "tool_calls"/"actions" key has arrived); a
      // partial legit object without those keys keeps typing out.
      if (/"tool_calls"\s*:|"actions"\s*:/.test(trimmed)) return "";
    }
  }
  return out;
}

// Display-only unwrap of the raw planner/turn envelope.
//
// The agent/gateway sometimes delivers a finished turn as a raw JSON
// envelope instead of plain reply text, e.g.
//   json {"status": "done", "summary": "Halo! Kabar saya baik, ..."}
// or the same object inside a ```json fence. The backend already knows
// how to extract `summary` (muse-agent parser / chat_turn), but the
// frontend stored and displayed the raw text. These helpers strip the
// envelope at display/finish time ONLY — they never touch the agent
// loop's parsing of actions, approvals, or turn status.
//
// Rule: unwrap only when the parsed object has BOTH a string `status`
// and a string `summary`. Anything else (plain text, a legit JSON code
// block the user asked for, a partial stream) passes through untouched.
export function unwrapReplyEnvelope(text: string): string {
  const stripped = stripToolCallBlocks(text);
  const s = stripped.trim();
  let inner: string | null = null;
  if (s.startsWith("```")) {
    // ```json ... ``` fence (closed only — an unclosed fence means the
    // stream is still arriving, leave it alone).
    const m = /^```(?:json)?[ \t]*\r?\n?([\s\S]*?)\r?\n?```\s*$/.exec(s);
    if (!m) return stripped;
    inner = m[1].trim();
  } else if (/^json\s*\{/.test(s)) {
    inner = s.replace(/^json\s*/, "");
  } else {
    return stripped;
  }
  try {
    const obj: unknown = JSON.parse(inner);
    if (obj && typeof obj === "object" && !Array.isArray(obj)) {
      const rec = obj as Record<string, unknown>;
      if (typeof rec.status === "string" && typeof rec.summary === "string") {
        return rec.summary;
      }
    }
  } catch {
    /* not JSON — fall through and return the text unchanged */
  }
  return stripped;
}

/** True while `text` looks like a raw envelope still streaming in —
// the JSON object has not closed yet, so it cannot be unwrapped. */
export function isEnvelopeStreaming(text: string): boolean {
  const s = text.trimStart();
  if (/^json\s*\{\s*"status"\s*:/.test(s)) return true;
  if (s.startsWith("```json")) {
    const inner = s.slice("```json".length);
    return /"status"\s*:/.test(inner) && /"summary"\s*:/.test(inner);
  }
  return false;
}

/** Display text for the live stream: strip tool-call JSON first (it
 * must never render, streaming or not), hide a raw envelope while it is
 * still arriving (it appears as the clean summary once it parses);
 * anything else passes through, unwrapped when complete. */
export function streamingReplyDisplay(text: string): string {
  // Strip tool-call blocks first, then unwrap: a COMPLETE envelope also
  // matches the "still streaming" prefix, so it must be tried before
  // the hide check.
  const stripped = stripToolCallBlocks(text);
  const unwrapped = unwrapReplyEnvelope(stripped);
  if (unwrapped !== stripped) return unwrapped;
  if (isEnvelopeStreaming(stripped)) return "";
  return stripped;
}
