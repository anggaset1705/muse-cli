/**
 * Playback order for a freshly generated assistant turn:
 * prose completes before the code of its Ran actions starts.
 * Archived messages must remain immediately visible on revisit.
 */
export function ranCodeSequence(
  currentTurn: boolean,
  streaming: boolean,
  proseTyping: boolean,
): { animate: boolean; defer: boolean } {
  return {
    animate: currentTurn,
    defer: currentTurn && (streaming || proseTyping),
  };
}

/**
 * Hold the entire Ran result (not merely its LiveCode content) while
 * assistant prose is still streaming or catching up. Pending approvals
 * remain visible so the user is never prevented from authorizing work.
 * Saved history is never delayed or replayed.
 */
export function shouldDeferRanCards(
  currentTurn: boolean,
  streaming: boolean,
  proseTyping: boolean,
  approvalPending: boolean,
): boolean {
  return currentTurn && (streaming || proseTyping) && !approvalPending;
}
