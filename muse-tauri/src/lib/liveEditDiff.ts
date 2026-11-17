/** Changed line markers computed from the REAL prepared before/after source.
 * No speculative edit events, mock progress, or guessed replacements.
 * Uses bounded LCS for small/medium files; for huge files marks the
 * common-prefix/suffix-delimited changed region without freezing UI. */
export interface ChangedLineMarkers {
  added: number[];
  modified: number[];
  removed: number;
}
function pushSpan(result: ChangedLineMarkers, oldFrom: number, oldTo: number, newFrom: number, newTo: number): void {
  const oldCount = oldTo - oldFrom;
  const newCount = newTo - newFrom;
  result.removed += oldCount;
  for (let i = 0; i < newCount; i++) {
    (i < oldCount ? result.modified : result.added).push(newFrom + i + 1);
  }
}
export function changedLineMarkers(before: string, after: string): ChangedLineMarkers {
  const oldLines = before.replace(/\r\n/g, "\n").split("\n");
  const newLines = after.replace(/\r\n/g, "\n").split("\n");
  const result: ChangedLineMarkers = { added: [], modified: [], removed: 0 };
  if (before === after) return result;
  let prefix = 0;
  while (prefix < oldLines.length && prefix < newLines.length && oldLines[prefix] === newLines[prefix]) prefix++;
  let oldTail = oldLines.length;
  let newTail = newLines.length;
  while (oldTail > prefix && newTail > prefix && oldLines[oldTail - 1] === newLines[newTail - 1]) {
    oldTail--;
    newTail--;
  }
  const n = oldTail - prefix;
  const m = newTail - prefix;
  if (n === 0 || m === 0 || n * m > 240_000) {
    pushSpan(result, prefix, oldTail, prefix, newTail);
    return result;
  }
  const stride = m + 1;
  const dp = new Uint32Array((n + 1) * stride);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i * stride + j] = oldLines[prefix + i] === newLines[prefix + j]
        ? 1 + dp[(i + 1) * stride + j + 1]
        : Math.max(dp[(i + 1) * stride + j], dp[i * stride + j + 1]);
    }
  }
  let oi = 0;
  let nj = 0;
  let oldStart = prefix;
  let newStart = prefix;
  while (oi < n && nj < m) {
    if (oldLines[prefix + oi] === newLines[prefix + nj]) {
      pushSpan(result, oldStart, prefix + oi, newStart, prefix + nj);
      oi++;
      nj++;
      oldStart = prefix + oi;
      newStart = prefix + nj;
    } else if (dp[(oi + 1) * stride + nj] >= dp[oi * stride + nj + 1]) {
      oi++;
    } else {
      nj++;
    }
  }
  pushSpan(result, oldStart, oldTail, newStart, newTail);
  return result;
}
