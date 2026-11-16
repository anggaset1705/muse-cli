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
  if (following && !userInitiated) return true;
  return isNearScrollBottom(metrics, threshold);
}
