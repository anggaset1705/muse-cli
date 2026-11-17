// A visual playback of the REAL validated patch delivered by Muse's
// native executor. The old file remains visible; only changed hunks
// are replaced in place, from top to bottom. Disk writes are atomic:
// this is an honest edit preview, NOT simulated filesystem keystrokes.
export interface InlineEditHunk {
  beforeStart: number;
  beforeEnd: number;
  inserted: string;
}
export interface InlineEditPlan {
  before: string;
  after: string;
  hunks: InlineEditHunk[];
  work: number;
}
export interface InlineEditFrame {
  text: string;
  focusLine: number | null;
  done: boolean;
  appliedHunks: number;
}

function linesWithEndings(s: string): string[] {
  // Preserve every newline and CRLF byte sequence, including EOF.
  return s.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

export function planInlineEdit(before: string, after: string): InlineEditPlan {
  if (before === after) return { before, after, hunks: [], work: 0 };
  const oldLines = linesWithEndings(before);
  const newLines = linesWithEndings(after);
  const n = oldLines.length;
  const m = newLines.length;
  let oldStart = 0;
  while (oldStart < n && oldStart < m && oldLines[oldStart] === newLines[oldStart]) oldStart++;
  let oldEnd = n;
  let newEnd = m;
  while (oldEnd > oldStart && newEnd > oldStart
    && oldLines[oldEnd - 1] === newLines[newEnd - 1]) {
    oldEnd--;
    newEnd--;
  }

  const oldOffsets = new Array<number>(n + 1).fill(0);
  const newOffsets = new Array<number>(m + 1).fill(0);
  for (let i = 0; i < n; i++) oldOffsets[i + 1] = oldOffsets[i] + oldLines[i].length;
  for (let j = 0; j < m; j++) newOffsets[j + 1] = newOffsets[j] + newLines[j].length;
  const remainingOld = oldEnd - oldStart;
  const remainingNew = newEnd - oldStart;
  const hunks: InlineEditHunk[] = [];
  const emit = (i0: number, i1: number, j0: number, j1: number) => {
    if (i0 === i1 && j0 === j1) return;
    hunks.push({
      beforeStart: oldOffsets[i0],
      beforeEnd: oldOffsets[i1],
      inserted: after.slice(newOffsets[j0], newOffsets[j1]),
    });
  };

  // Exact LCS for ordinary source files; bounded memory for large files.
  if (remainingOld * remainingNew <= 260_000) {
    const stride = remainingNew + 1;
    const dp = new Uint32Array((remainingOld + 1) * stride);
    for (let i = remainingOld - 1; i >= 0; i--) {
      for (let j = remainingNew - 1; j >= 0; j--) {
        dp[i * stride + j] = oldLines[oldStart + i] === newLines[oldStart + j]
          ? 1 + dp[(i + 1) * stride + j + 1]
          : Math.max(dp[(i + 1) * stride + j], dp[i * stride + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    let rangeOld = oldStart;
    let rangeNew = oldStart;
    while (i < remainingOld && j < remainingNew) {
      if (oldLines[oldStart + i] === newLines[oldStart + j]) {
        emit(rangeOld, oldStart + i, rangeNew, oldStart + j);
        i++;
        j++;
        rangeOld = oldStart + i;
        rangeNew = oldStart + j;
      } else if (dp[(i + 1) * stride + j] >= dp[i * stride + j + 1]) {
        i++;
      } else {
        j++;
      }
    }
    emit(rangeOld, oldEnd, rangeNew, newEnd);
  } else {
    // Avoid blocking the UI on huge inputs: one exact changed region.
    emit(oldStart, oldEnd, oldStart, newEnd);
  }
  return {
    before,
    after,
    hunks,
    work: hunks.reduce((sum, hunk) => sum + Math.max(1, hunk.inserted.length), 0),
  };
}

export function inlineEditFrame(plan: InlineEditPlan, workDone: number): InlineEditFrame {
  if (plan.work === 0 || workDone >= plan.work) {
    return { text: plan.after, focusLine: null, done: true, appliedHunks: plan.hunks.length };
  }
  let remaining = Math.max(0, Math.floor(workDone));
  let result = "";
  let cursor = 0;
  let applied = 0;
  let focusLine: number | null = null;
  for (const hunk of plan.hunks) {
    if (remaining <= 0) break;
    result += plan.before.slice(cursor, hunk.beforeStart);
    const cost = Math.max(1, hunk.inserted.length);
    const consume = Math.min(remaining, cost);
    result += hunk.inserted.slice(0, consume);
    cursor = hunk.beforeEnd;
    remaining -= consume;
    focusLine = result.split("\n").length;
    if (consume < cost) {
      result += plan.before.slice(cursor);
      return { text: result, focusLine, done: false, appliedHunks: applied };
    }
    applied++;
  }
  result += plan.before.slice(cursor);
  return { text: result, focusLine, done: false, appliedHunks: applied };
}
