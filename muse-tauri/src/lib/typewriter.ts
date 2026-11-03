// Typewriter engine (A3, port of WinForms ChatTranscript: 16 ms tick,
// 3 chars/tick). Pure functions — no React — so the tick logic can be
// unit-tested in Node; ChatPanel's interval just calls typewriterStep().
//
// The displayed text chases the target text. Two rules:
//
// 1. Never cut inside an unclosed code fence. An odd count of ``` in the
//    cut means we are inside a fence, so hold at the fence opener until
//    the fence closes and the markdown render never breaks mid-block.
// 2. The animation finishes when displayed has caught up with the
//    target — NOT when the streaming flag flips. The agent often delivers
//    a whole reply in a single delta and finish() fires right after, so
//    stopping on the flag would make the reply appear all at once. A cut
//    held at a fence opener waits while the fence is still open in the
//    target; once the fence has closed further along, the whole block
//    comes in at once and typing continues after it — never mid-block,
//    and never stuck: the cut window alone can never see the closing
//    fence, so the hold is always judged against the full target. When
//    the turn is over and the fence never closes, the full text is shown
//    rather than truncating the reply forever.

export function fenceCount(text: string): number {
  return (text.match(/```/g) ?? []).length;
}

export function safeCut(target: string, n: number): string {
  let cut = target.slice(0, n);
  if (fenceCount(cut) % 2 === 1) {
    const last = cut.lastIndexOf("```");
    // Hold at the fence opener (last === 0 counts too: a reply that
    // starts with a code block shows nothing until the fence closes).
    if (last >= 0) cut = cut.slice(0, last);
  }
  // A cut can also end 1–2 chars into a fence opener (a 3-char step
  // lands on a partial ```). Without this, displayed would end with a
  // lone backtick and the next cut would hold BEFORE it forever — the
  // animation deadlocks, because the cut window is anchored at the
  // stuck position and never grows past the fence. Back off past the
  // partial marker; only a run of exactly 1–2 is trimmed, a run of 3+
  // is a complete fence marker and stays.
  let trailing = 0;
  while (trailing < cut.length && cut[cut.length - 1 - trailing] === "`") trailing++;
  if (trailing === 1 || trailing === 2) cut = cut.slice(0, cut.length - trailing);
  return cut;
}

/** Chars added per 16 ms tick. 3 chars/tick like muse.exe, so a ~90 char
 * reply takes ~0.5 s to type out and stays visibly live; catch up
 * faster when far behind so a long reply does not take minutes. */
export function typewriterStepSize(gap: number): number {
  if (gap > 2000) return 60;
  if (gap > 600) return 30;
  return 3;
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
 * - Returns `current` unchanged when the animation must wait (held at an
 *   unclosed fence while still streaming) or is already done.
 */
export function typewriterStep(target: string, current: string, streaming: boolean): string {
  if (current.length >= target.length) return current;
  if (current.length > 0 && !target.startsWith(current)) return target;
  const gap = target.length - current.length;
  const next = safeCut(target, current.length + typewriterStepSize(gap));
  if (next.length > current.length) return next;
  // Held at a fence opener. The cut window (current + one step) never
  // reaches the closing fence, so "wait for the fence to close" has to
  // be judged against the FULL target, not the window: if the fence has
  // closed further along, bring the whole block in at once and keep
  // typing after it. If it has not closed yet, wait while streaming;
  // once the turn is over it never closes, so show the full text rather
  // than truncating the reply forever.
  const opener = target.indexOf("```", current.length);
  if (opener === -1) return target;
  const closer = target.indexOf("```", opener + 3);
  if (closer !== -1) return target.slice(0, closer + 3);
  return streaming ? current : target;
}
