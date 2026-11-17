/** Only newly growing code should auto-follow in a Markdown card.
 * Stored old messages are NOT scrolled to their end when opened.
 * Once a live code card grows, this policy is independent of the outer
 * chat viewport: scrolling the chat does not disable code auto-follow.
 * The user's own scroll inside the code card is separately authoritative. */
export function isLiveCodeGrowth(previous: string | null, current: string, liveTyping: boolean): boolean {
  if (liveTyping) return true;
  return previous !== null && current.length > previous.length && current.startsWith(previous);
}
