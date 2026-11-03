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
  const s = text.trim();
  let inner: string | null = null;
  if (s.startsWith("```")) {
    // ```json ... ``` fence (closed only — an unclosed fence means the
    // stream is still arriving, leave it alone).
    const m = /^```(?:json)?[ \t]*\r?\n?([\s\S]*?)\r?\n?```\s*$/.exec(s);
    if (!m) return text;
    inner = m[1].trim();
  } else if (/^json\s*\{/.test(s)) {
    inner = s.replace(/^json\s*/, "");
  } else {
    return text;
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
  return text;
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

/** Display text for the live stream: hide a raw envelope while it is
 * still arriving (it appears as the clean summary once it parses);
 * anything else passes through, unwrapped when complete. */
export function streamingReplyDisplay(text: string): string {
  // Unwrap first: a COMPLETE envelope also matches the "still
  // streaming" prefix, so it must be tried before the hide check.
  const unwrapped = unwrapReplyEnvelope(text);
  if (unwrapped !== text) return unwrapped;
  if (isEnvelopeStreaming(text)) return "";
  return text;
}
