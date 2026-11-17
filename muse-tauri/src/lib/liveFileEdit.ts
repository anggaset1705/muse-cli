/** A verified, prepared overwrite/patch of an EXISTING project file.
 * The backend emits the real before/after bytes, then a tool_result
 * confirms whether its atomic write succeeded. The editor is a read-only
 * preview, not a claim that each character was written to disk. */
export interface LiveFileEdit {
  path: string;
  /** Monotonic per-window edit identity (two edits may target same file). */
  revision: number;
  tool: "edit_file" | "write_file";
  before: string;
  after: string;
  diff: string;
  status: "preparing" | "applied" | "failed";
  result?: string;
}
