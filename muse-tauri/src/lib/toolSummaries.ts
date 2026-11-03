// Friendly per-tool summaries — port of the WinForms strings used by
// ChatTranscript.ToolCallCard (FormatToolSummary) and
// DesktopCopy.Activity (UiStrings.cs Tool*Running/Tool*Done,
// Activity*). Used for the live status line, the thinking card, and
// the tool cards in ChatPanel.

import type { Lang } from "./i18n";

export type ToolKind =
  | "read_file"
  | "read_files"
  | "write_file"
  | "edit_file"
  | "search_files"
  | "list_files"
  | "run_command"
  | "git_status"
  | "git_diff"
  | "process_status"
  | "process_stop"
  | "process_input"
  | "skill"
  | "computer";

/** Emoji per tool, as in ChatTranscript.GetToolIcon (WinForms). */
export function toolIcon(tool: string): string {
  const t = tool.toLowerCase();
  if (t.startsWith("read_file") || t === "read_files") return "📖";
  if (t.startsWith("write_file") || t.startsWith("edit_file")) return "✎";
  if (t.startsWith("search_files")) return "🔍";
  if (t.startsWith("list_files")) return "📁";
  if (t.startsWith("git_")) return "🔀";
  if (t.startsWith("run_command") || t.startsWith("process_")) return "🖥️";
  if (t === "skill") return "✨";
  if (t.startsWith("computer_")) return "🖥️";
  return "🔧";
}

function fileName(path?: string): string {
  if (!path) return "";
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

function fmt(template: string, file: string): string {
  return template.replace("{0}", file);
}

/** "Membaca file X…" while running / "Sudah membaca X" when done. */
export function toolSummary(
  tool: string,
  path: string | undefined,
  description: string | undefined,
  done: boolean,
  lang: Lang,
): string {
  const t = tool.toLowerCase();
  const file = fileName(path) || fileName(description);
  const id = lang === "id";
  const run = (en: string, enGeneric: string, idStr: string, idGeneric: string) =>
    id ? fmt(file ? idStr : idGeneric, file) : fmt(file ? en : enGeneric, file);
  const fin = (en: string, enGeneric: string, idStr: string, idGeneric: string) =>
    id ? fmt(file ? idStr : idGeneric, file) : fmt(file ? en : enGeneric, file);
  switch (true) {
    case t.startsWith("read_file"):
      return done
        ? fin("Read {0}", "Read a file", "Sudah membaca {0}", "Sudah membaca file")
        : run("Reading {0}", "Reading a file", "Membaca {0}", "Membaca file");
    case t.startsWith("write_file"):
      return done
        ? fin("Wrote {0}", "Wrote a file", "Sudah menulis {0}", "Sudah menulis file")
        : run("Writing {0}", "Writing a file", "Menulis {0}", "Menulis file");
    case t.startsWith("edit_file"):
      return done
        ? fin("Edited {0}", "Edited a file", "Sudah mengubah {0}", "Sudah mengubah file")
        : run("Editing {0}", "Editing a file", "Mengubah {0}", "Mengubah file");
    case t.startsWith("search_files"):
      return done ? (id ? "Sudah mencari di proyek" : "Searched the project") : id ? "Mencari di proyek" : "Searching the project";
    case t.startsWith("list_files"):
      return id ? "Menjelajahi struktur proyek" : "Exploring the project structure";
    case t.startsWith("git_"):
      return done ? (id ? "Sudah memeriksa perubahan Git" : "Checked Git changes") : id ? "Memeriksa perubahan Git" : "Checking Git changes";
    case t.startsWith("run_command"): {
      const cmd = (description ?? "").trim();
      if (done) return id ? `Sudah menjalankan perintah${cmd ? `: ${cmd}` : ""}` : `Ran command${cmd ? `: ${cmd}` : ""}`;
      if (/test/i.test(cmd)) return id ? "Menjalankan pengujian proyek" : "Running project tests";
      if (/build/i.test(cmd)) return id ? "Memeriksa build proyek" : "Checking the project build";
      return id ? `Menjalankan perintah${cmd ? `: ${cmd}` : ""}` : `Running command${cmd ? `: ${cmd}` : ""}`;
    }
    case t.startsWith("process_"):
      return id ? "Memeriksa proses" : "Checking process";
    case t === "skill":
      return description ?? (id ? "Memakai skill" : "Using a skill");
    case t.startsWith("computer_"):
      return id ? "Mengendalikan layar" : "Controlling the screen";
    default:
      return description ?? (id ? "Memproses permintaan" : "Processing the request");
  }
}

/** One-line live activity for the status line (DesktopCopy.Activity). */
export function activityLine(tool: string, path: string | undefined, detail: string, lang: Lang): string {
  return toolSummary(tool, path, detail, false, lang);
}
