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
//    stopping on the flag would make the reply appear all at once. While
//    streaming, a held cut just waits; once the turn is over the fence
//    will never close, so a held cut jumps to the full target instead of
//    truncating the reply forever (e.g. a reply with an unclosed fence).
export function fenceCount(text) {
    return (text.match(/```/g) ?? []).length;
}
export function safeCut(target, n) {
    let cut = target.slice(0, n);
    if (fenceCount(cut) % 2 === 1) {
        const last = cut.lastIndexOf("```");
        if (last > 0)
            cut = cut.slice(0, last);
    }
    return cut;
}
/** Chars added per 16 ms tick. 3 chars/tick like muse.exe, so a ~90 char
 * reply takes ~0.5 s to type out and stays visibly live; catch up
 * faster when far behind so a long reply does not take minutes. */
export function typewriterStepSize(gap) {
    if (gap > 2000)
        return 60;
    if (gap > 600)
        return 30;
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
export function typewriterStep(target, current, streaming) {
    if (current.length >= target.length)
        return current;
    if (current.length > 0 && !target.startsWith(current))
        return target;
    const gap = target.length - current.length;
    const next = safeCut(target, current.length + typewriterStepSize(gap));
    if (next.length <= current.length) {
        // Held at an unclosed fence: wait for it to close while streaming;
        // once the turn is over it never closes, so show the full text.
        return streaming ? current : target;
    }
    return next;
}
