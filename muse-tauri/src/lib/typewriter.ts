// Typewriter engine (A3, port of WinForms ChatTranscript: 24 ms tick,
// 2-4 chars/tick). Pure functions — no React — so the tick logic can be
// unit-tested in Node; ChatPanel's interval just calls typewriterStep().
//
// MIRROR: typewriter.js next to this file is the copy Vite bundles
// (`@/lib/typewriter` resolves to the .js first) — any change here MUST
// be mirrored there, or the shipped app silently runs different logic
// than the tests exercise.
//
// The displayed text chases the target text. Rules (13 Nov 2026 fix):
//
// 1. The chase types THROUGH code fences, character by character —
//    it never holds at a fence opener and never jumps a whole block in
//    at once. (The old rule held at the opener until the fence closed
//    and then brought the whole block in at once: a reply whose code
//    block arrived in a single delta therefore appeared whole in one
//    tick — Bree's 18:31 live test: the 1584-char reply finished in
//    25 ticks = 0.40 s with a 1383-char single-tick jump.) MessageView
//    splits on ``` and renders a partial fence as a CodeBlock with the
//    partial code, so typing mid-block never breaks the render.
// 2. Step sizes stay small enough to stay visibly live: a ~1000-char
//    reply takes several seconds to type out, not ~1 s; a ~6 KB file
//    still finishes in seconds, not minutes. See typewriterStepSize.
// 3. The animation finishes when displayed has caught up with the
//    target — NOT when the streaming flag flips. The agent often delivers
//    a whole reply in a single delta and finish() fires right after, so
//    stopping on the flag would make the reply appear all at once.

export const TYPEWRITER_TICK_MS = 24;

export function fenceCount(text: string): number {
  return (text.match(/```/g) ?? []).length;
}

/** Cut `target` to `n` chars, backing off a trailing run of exactly
 * 1–2 backticks so displayed never ends on a partial ``` marker.
 * (Kept as a helper; the engine no longer holds at fence openers.) */
export function safeCut(target: string, n: number): string {
  let cut = target.slice(0, n);
  let trailing = 0;
  while (trailing < cut.length && cut[cut.length - 1 - trailing] === "`") trailing++;
  if (trailing === 1 || trailing === 2) cut = cut.slice(0, cut.length - trailing);
  return cut;
}

/** Shared visible reveal pace: 2/3/4 characters per 24 ms tick.
 * Normal short chat replies become twice as responsive; the live existing-
 * file diff editor now uses this same bounded pace instead of revealing
 * 12-120 characters every 32 ms. This is display only, not disk I/O. */
export function typewriterStepSize(gap: number): number {
  if (gap > 2000) return 4;
  if (gap > 600) return 3;
  return 2;
}

/**
 * One 16 ms tick of the live code typing inside a "Ran ..." card (13 Nov
 * 2026). Same chase as typewriterStep: the target is raw file content,
 * not markdown, so there is no fence to hold at — plain prefix growth.
 * Step sizes are the same tiers as typewriterStepSize, so a ~6 KB file
 * types out in ~8 s and stays visibly live instead of appearing whole;
 * a far-behind target still catches up instead of taking minutes.
 */
export function liveContentStep(target: string, current: string): string {
  if (current.length >= target.length) return current;
  if (current.length > 0 && !target.startsWith(current)) return target;
  const next = current.length + typewriterStepSize(target.length - current.length);
  return target.slice(0, Math.min(next, target.length));
}

/**
 * One 16 ms tick of the chase. Returns the next displayed text.
 *
 * - `target` is the full text to reach (live `streamingText` while the
 *   turn runs, the saved message content once it finished).
 * - `current` is what is displayed now; it must be a prefix of `target`
 *   (or ""), otherwise the target was replaced mid-turn (e.g. a fallback
 *   note prepended at finish) and we jump to the full target rather than
 *   show a garbled mix.
 * - The chase types straight through code fences: every tick adds at
 *   most one step (3–12 chars), so a fenced code block grows
 *   progressively instead of appearing whole. `_streaming` is kept only
 *   for signature compatibility with the old callers.
 */
export function typewriterStep(target: string, current: string, _streaming: boolean): string {
  void _streaming;
  // A provisional step may be replaced by the assistant final message.
  // Drop the stale prefix; subsequent ticks type the new text normally.
  if (current.length > 0 && !target.startsWith(current)) return "";
  if (current.length >= target.length) return current;
  const gap = target.length - current.length;
  return target.slice(0, current.length + typewriterStepSize(gap));
}
