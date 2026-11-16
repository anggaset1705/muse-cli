// "Ran ..." card helpers (Hermes pattern) + exit-code parsing for tool
// results. Pure functions — no React — so they are unit-tested in Node
// (see scripts/test-ui-hermes.mjs).
//
// Exit code source: the gateway already puts `exit_code=N` in the
// metadata of a run_command result (muse-gateway processes.rs returns
// `exit_code=…\nstdout:\n…\nstderr:\n…`), and tool_outcome.rs parses the
// same field server-side — so the frontend parses the same text instead
// of needing a new gateway event. Only the metadata (before the first
// `stdout:` / `stderr:` / `output (` boundary) is read, so an
// `exit_code=0` printed by the command itself is never mistaken for
// the real exit code.

/** Parse the exit code out of a tool result string, if it reports one. */
export function parseExitCode(result?: string): number | undefined {
  if (!result) return undefined;
  const boundary = /^(?:stdout:|stderr:|output \()/m.exec(result);
  const meta = boundary ? result.slice(0, boundary.index) : result;
  const m = /^exit_code=(-?\d+)\s*$/m.exec(meta);
  return m ? Number(m[1]) : undefined;
}

/** Card title, Hermes-style: the command itself for run_command. */
export function ranTitle(tool: string, description?: string): string {
  const t = tool.toLowerCase();
  if (t.startsWith("run_command")) {
    const cmd = (description ?? "").replace(/^run_command:\s*/i, "").trim();
    return cmd ? `Ran ${cmd}` : "Ran command";
  }
  if (t === "skill") {
    const name = (description ?? "").trim();
    return name ? `Ran skill: ${name}` : "Ran skill";
  }
  return `Ran ${tool}`;
}

/** The visible command / argument line under the card title. */
export function ranCommandLine(
  tool: string,
  description?: string,
  path?: string,
): string {
  const t = tool.toLowerCase();
  if (t.startsWith("run_command")) {
    return (description ?? "").replace(/^run_command:\s*/i, "").trim();
  }
  if (path) return path;
  return (description ?? "").trim();
}

/** Only the plain success receipt from write_file is redundant when the
 * card already displays the file name, content and done status. Never
 * hide failures, warnings, results from other tools, or real diagnostics.
 * This changes presentation only; action.result is still stored. */
export function shouldShowRanOutput(tool: string, status: string, result?: string): boolean {
  if (!result) return false;
  const receiptOnly =
    tool.trim().toLowerCase() === "write_file" &&
    status === "done" &&
    /^wrote\s+\S.*\s+\(\d+\s+chars?\)$/i.test(result.trim());
  return !receiptOnly;
}