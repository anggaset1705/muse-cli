/** Preserve live follow when content grows on its own. Only intentional
 * scrolling may pause it; scrolling back to the bottom resumes following.
 * Shared by chat transcript and nested Ran code window. */
export interface ScrollMetrics {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

export function isNearScrollBottom(metrics: ScrollMetrics, threshold = 64): boolean {
  return metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight <= threshold;
}

export function reconcileScrollFollow(
  following: boolean,
  userInitiated: boolean,
  metrics: ScrollMetrics,
  threshold = 64,
): boolean {
  // A programmatic scroll event after a layout update may NOT undo a
  // deliberate pause. A paused viewport resumes only after a genuine
  // user scroll reaches the actual bottom, not merely "near" it.
  if (!userInitiated) return following;
  return isNearScrollBottom(metrics, following ? threshold : 2);
}
