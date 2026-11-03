// Right workspace panel: Files / Changes / Terminal / Screen tabs.
// Files + Terminal run LOCALLY via Rust commands, scoped to the chosen
// workspace folder. Screen control is REAL: the tab drives the gateway's
// /v1/agent/screen/* endpoints (list_windows, authorize, preview,
// disarm, auto_authorize) and shares the screen session with the agent
// loop, so a window picked here becomes the agent's computer_inspect
// target too (like the WinForms ComputerScopeForm + Screen tab).
// Changes made by the agent loop are listed too (by: "agent") and undone
// server-side via POST /v1/agent/sessions/{id}/revert.
//
// Ported from WinForms: lazy-expand tree with "…" placeholder (C4),
// dirty indicator + discard confirm (C8, incl. close-app confirm via the
// Rust on_window_event guard), concurrency check on save
// (C7, re-read before write), CRLF preservation, "Ask Muse" (C9),
// unified diff in Changes (C11), background terminal + send result to
// the agent (D2/D5), output cap (D4), Ctrl+S / Ctrl+Tab (K2).
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ChevronDown,
  ChevronRight,
  File,
  FileArchive,
  FileCode2,
  FileImage,
  FileJson,
  FileText,
  Folder,
  FolderOpen,
  MessageSquarePlus,
  Monitor,
  RefreshCw,
  Save,
  Square,
  Play,
  Send,
  Undo2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CodeEditor } from "@/components/CodeEditor";
import type { AgentScreenStatus, DesktopWindow, FileChange, FileEntry, Settings } from "@/lib/types";
import { formatSize } from "@/lib/types";
import {
  autoAuthorizeAgentScreen,
  authorizeAgentScreenWindow,
  disarmAgentScreen,
  fetchAgentScreenPreview,
  fetchAgentScreenStatus,
  fetchAgentScreenWindows,
} from "@/lib/gateway";
import { t, tf, type Lang } from "@/lib/i18n";

export type WorkspaceTab = "files" | "changes" | "terminal" | "screen";

interface Props {
  lang: Lang;
  settings: Settings;
  /** Live gateway base URL (App's effectiveBase: backend.base_url while
   *  the sidecar runs, else settings.backendBaseUrl). The Screen tab must
   *  call the RUNNING gateway — the settings default is a fixed legacy
   *  port and is wrong for the Rust sidecar on a random port. */
  backendBaseUrl: string;
  changes: FileChange[];
  onRecordChange: (c: FileChange) => void;
  onUndoChange: (c: FileChange) => void;
  /** Server-side revert for agent-made changes (POST /v1/agent/.../revert). */
  onRevertAgentChange: (c: FileChange) => Promise<void>;
  /** Bumped by App after an agent turn so the file tree refreshes. */
  refreshSignal: number;
  onShellChange: (shell: string) => void;
  onCollapsedChange: (collapsed: boolean) => void;
  collapsed: boolean;
  /** Tab is controlled by App so "View terminal" (A5) can switch it. */
  tab: WorkspaceTab;
  onTabChange: (tab: WorkspaceTab) => void;
  /** "Ask Muse" (C9): prefill the chat composer with a prompt about this file. */
  onAskMuse: (prompt: string) => void;
  /** Send terminal output to the agent as context (D5). */
  onSendToAgent: (text: string) => void;
  /** A local editor save — prepended to the next agent prompt as context (C13). */
  onNoteEditorChange: (rel: string) => void;
  /** C8 — fired whenever the editor dirty state changes; App mirrors it
   *  to the Rust close guard (set_editor_dirty) so closing the app asks
   *  before discarding unsaved edits. */
  onDirtyChange?: (dirty: boolean) => void;
}

const MAX_DISPLAY_OUTPUT = 200_000; // D4: WinForms trims the console at 200k

function updateTree(entries: FileEntry[], rel: string, children: FileEntry[]): FileEntry[] {
  return entries.map((e) => {
    if (e.rel === rel) return { ...e, children };
    if (e.children) return { ...e, children: updateTree(e.children, rel, children) };
    return e;
  });
}

function FileTree({
  entries,
  depth,
  openDirs,
  loadingDirs,
  onToggleDir,
  onOpen,
}: {
  entries: FileEntry[];
  depth: number;
  openDirs: Record<string, boolean>;
  loadingDirs: Record<string, boolean>;
  onToggleDir: (f: FileEntry) => void;
  onOpen: (f: FileEntry) => void;
}) {
  return (
    <div>
      {entries.map((e) => {
        const isOpen = !!openDirs[e.rel];
        if (e.is_dir) {
          return (
            <div key={e.rel}>
              <button
                className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-[12.5px] text-[#A1A1AA] hover:bg-[#292A34] hover:text-white"
                style={{ paddingLeft: `${6 + depth * 14}px` }}
                onClick={() => onToggleDir(e)}
                aria-expanded={isOpen}
              >
                {isOpen ? <ChevronDown className="h-3.5 w-3.5 shrink-0" /> : <ChevronRight className="h-3.5 w-3.5 shrink-0" />}
                {isOpen ? <FolderOpen className="h-4 w-4 shrink-0 text-[#8aa8ff]" /> : <Folder className="h-4 w-4 shrink-0 text-[#8aa8ff]" />}
                <span className="min-w-0 flex-1 truncate">{e.name}</span>
                {loadingDirs[e.rel] && <span className="text-[11px] text-[#71717A]">…</span>}
              </button>
              {isOpen && (
                e.children === null ? (
                  <div className="py-0.5 pl-8 text-[12px] text-[#71717A]" style={{ paddingLeft: `${6 + (depth + 1) * 14 + 14}px` }}>…</div>
                ) : (
                  <FileTree entries={e.children ?? []} depth={depth + 1} openDirs={openDirs} loadingDirs={loadingDirs} onToggleDir={onToggleDir} onOpen={onOpen} />
                )
              )}
            </div>
          );
        }
        return (
          <button
            key={e.rel}
            className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-[12.5px] text-[#A1A1AA] hover:bg-[#292A34] hover:text-white"
            style={{ paddingLeft: `${6 + depth * 14 + 14}px` }}
            onClick={() => onOpen(e)}
            title={e.rel}
          >
            {fileIcon(e.name)}
            <span className="min-w-0 flex-1 truncate">{e.name}</span>
            <span className="shrink-0 font-mono text-[10px] text-[#71717A]">{formatSize(e.size)}</span>
          </button>
        );
      })}
    </div>
  );
}

function toLineEndings(text: string, useCrlf: boolean): string {
  const lf = text.replace(/\r\n/g, "\n");
  return useCrlf ? lf.replace(/\n/g, "\r\n") : lf;
}

/** K10 — one Lucide icon per file kind (WinForms drew 14 icons by hand);
 *  folders keep Folder/FolderOpen. Extension lookup is case-insensitive. */
function fileIcon(name: string) {
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  switch (ext) {
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
    case "mjs":
    case "cjs":
    case "py":
    case "rs":
    case "cs":
    case "go":
    case "java":
    case "c":
    case "cpp":
    case "h":
    case "css":
    case "scss":
    case "html":
    case "xml":
    case "yml":
    case "yaml":
    case "toml":
    case "sh":
    case "ps1":
      return <FileCode2 className="h-4 w-4 shrink-0 text-[#7aa2ff]" />;
    case "json":
    case "jsonc":
      return <FileJson className="h-4 w-4 shrink-0 text-[#e5c07b]" />;
    case "md":
    case "markdown":
    case "txt":
      return <FileText className="h-4 w-4 shrink-0 text-[#A1A1AA]" />;
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "svg":
    case "bmp":
    case "ico":
      return <FileImage className="h-4 w-4 shrink-0 text-[#98c379]" />;
    case "zip":
    case "tar":
    case "gz":
    case "7z":
    case "rar":
      return <FileArchive className="h-4 w-4 shrink-0 text-[#c678dd]" />;
    default:
      return <File className="h-4 w-4 shrink-0 text-[#71717A]" />;
  }
}

export function WorkspacePanel(props: Props) {
  const { lang, settings, tab, onTabChange, backendBaseUrl } = props;
  const root = settings.workspacePath;
  const [tree, setTree] = useState<FileEntry[]>([]);
  const [treeError, setTreeError] = useState<string | null>(null);
  const [treeLoading, setTreeLoading] = useState(false);
  const [openDirs, setOpenDirs] = useState<Record<string, boolean>>({});
  const [loadingDirs, setLoadingDirs] = useState<Record<string, boolean>>({});
  const [openFile, setOpenFile] = useState<{ rel: string; content: string } | null>(null);
  const [edited, setEdited] = useState("");
  const [baselineCrlf, setBaselineCrlf] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [fileChanged, setFileChanged] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  // Terminal state
  const [termCmd, setTermCmd] = useState("");
  const [termRunId, setTermRunId] = useState<number | null>(null);
  const [termOutput, setTermOutput] = useState("");
  const [termRunning, setTermRunning] = useState(false);
  const [termBackground, setTermBackground] = useState(false);
  const [termFinished, setTermFinished] = useState(false);
  const termOutRef = useRef<HTMLDivElement>(null);

  // Screen picker (live from the gateway; shares the agent loop's
  // screen session, so the authorized window is the agent's target too)
  const [screenStatus, setScreenStatus] = useState<AgentScreenStatus | null>(null);
  const [screenError, setScreenError] = useState<string | null>(null);
  const [screenLoading, setScreenLoading] = useState(false);
  const [screenWindows, setScreenWindows] = useState<DesktopWindow[]>([]);
  const [screenWindowsLoading, setScreenWindowsLoading] = useState(false);
  const [screenSearch, setScreenSearch] = useState("");
  const [screenTarget, setScreenTarget] = useState<DesktopWindow | null>(null);
  const [screenSelected, setScreenSelected] = useState<DesktopWindow | null>(null);
  const [livePreview, setLivePreview] = useState(true);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [screenActionError, setScreenActionError] = useState<string | null>(null);
  const [screenBusy, setScreenBusy] = useState(false);

  const screenReadonly = settings.turboMode === "readonly";

  // Live gateway URL for all Screen calls; fall back to the settings
  // default only when App has no live URL yet.
  const screenBase = backendBaseUrl || settings.backendBaseUrl;

  // Changes diff expansion
  const [openDiffs, setOpenDiffs] = useState<Record<string, boolean>>({});

  const isDirty = openFile !== null && edited !== openFile.content;

  // C8 — report dirty state upward so App can mirror it to the Rust
  // close guard (set_editor_dirty); closing the app then asks first.
  useEffect(() => {
    props.onDirtyChange?.(isDirty);
  }, [isDirty]);

  const refreshTree = useCallback(async () => {
    if (!root) return;
    setTreeLoading(true);
    setTreeError(null);
    try {
      const entries = await invoke<FileEntry[]>("list_workspace_files", { root, rel: "" });
      setTree(entries);
    } catch (e) {
      setTreeError(String(e));
      setTree([]);
    } finally {
      setTreeLoading(false);
    }
  }, [root]);

  useEffect(() => {
    void refreshTree();
  }, [refreshTree]);

  // Refresh the tree after an agent turn (App bumps refreshSignal).
  useEffect(() => {
    if (props.refreshSignal > 0) void refreshTree();
  }, [props.refreshSignal, refreshTree]);

  // Live screen status + window list from the gateway when the Screen
  // tab is opened (WinForms ComputerScopeForm: Search + Refresh + list).
  const loadScreen = useCallback(async () => {
    setScreenLoading(true);
    setScreenWindowsLoading(true);
    setScreenError(null);
    try {
      const st = await fetchAgentScreenStatus(screenBase, settings.apiKey);
      setScreenStatus(st);
    } catch (e) {
      setScreenStatus(null);
      setScreenError(String(e));
    } finally {
      setScreenLoading(false);
    }
    try {
      const res = await fetchAgentScreenWindows(screenBase, settings.apiKey);
      setScreenWindows(Array.isArray(res?.windows) ? res.windows : []);
      setScreenTarget(res?.target ?? null);
    } catch (e) {
      setScreenWindows([]);
      setScreenActionError(String(e));
    } finally {
      setScreenWindowsLoading(false);
    }
  }, [screenBase, settings.apiKey]);

  useEffect(() => {
    if (tab !== "screen") return;
    void loadScreen();
  }, [tab, loadScreen]);

  // Live preview: poll at most 1x per 1.5s while the tab is open, the
  // checkbox is on, and a window is authorized. Stops when the tab
  // closes or the checkbox is off (effect cleanup).
  useEffect(() => {
    if (tab !== "screen" || !livePreview || !screenTarget) {
      setPreviewUrl(null);
      return;
    }
    let cancelled = false;
    const tick = async () => {
      try {
        const p = await fetchAgentScreenPreview(screenBase, settings.apiKey);
        if (cancelled) return;
        setPreviewUrl(p.image_data_url);
        setPreviewError(null);
        // Keep the target in sync without recreating this effect on
        // every poll: only replace it when the handle actually changed.
        if (p.target) {
          setScreenTarget((prev) => (prev && prev.handle === p.target!.handle ? prev : p.target));
        }
      } catch (e) {
        if (!cancelled) setPreviewError(String(e));
      }
    };
    void tick();
    const id = window.setInterval(() => void tick(), 1500);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [tab, livePreview, screenTarget, screenBase, settings.apiKey]);

  const authorizeWindow = async (w: DesktopWindow) => {
    setScreenBusy(true);
    setScreenActionError(null);
    try {
      const r = await authorizeAgentScreenWindow(screenBase, settings.apiKey, w);
      setScreenTarget(r.target ?? w);
      setScreenSelected(w);
    } catch (e) {
      setScreenActionError(String(e));
    } finally {
      setScreenBusy(false);
    }
  };

  const disarmScreen = async () => {
    setScreenBusy(true);
    setScreenActionError(null);
    try {
      await disarmAgentScreen(screenBase, settings.apiKey);
      setScreenTarget(null);
      setScreenSelected(null);
      setPreviewUrl(null);
    } catch (e) {
      setScreenActionError(String(e));
    } finally {
      setScreenBusy(false);
    }
  };

  const autoAuthorizeScreen = async () => {
    setScreenBusy(true);
    setScreenActionError(null);
    try {
      const r = await autoAuthorizeAgentScreen(screenBase, settings.apiKey);
      if (r.target) {
        setScreenTarget(r.target);
        setScreenSelected(r.target);
      }
    } catch (e) {
      setScreenActionError(String(e));
    } finally {
      setScreenBusy(false);
    }
  };

  const filteredWindows = screenWindows.filter((w) => {
    const q = screenSearch.trim().toLowerCase();
    if (!q) return true;
    return (
      w.title.toLowerCase().includes(q) ||
      (w.application_name ?? "").toLowerCase().includes(q)
    );
  });

  // K2 — Ctrl+Tab cycles the detail tabs.
  useEffect(() => {
    const tabs: WorkspaceTab[] = ["files", "changes", "terminal", "screen"];
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "Tab") {
        e.preventDefault();
        const i = tabs.indexOf(tab);
        onTabChange(tabs[(i + 1) % tabs.length]);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tab, onTabChange]);

  useEffect(() => {
    if (termRunning && termRunId !== null) {
      const id = window.setInterval(async () => {
        try {
          const out = await invoke<{ id: number; output: string; running: boolean }>("terminal_output", { id: termRunId });
          setTermOutput(out.output);
          if (!out.running) {
            setTermRunning(false);
            setTermFinished(true);
            if (termBackground) setNotice(t(lang, "backgroundFinished"));
            window.clearInterval(id);
          }
        } catch {
          setTermRunning(false);
        }
      }, 500);
      return () => window.clearInterval(id);
    }
    return undefined;
  }, [termRunning, termRunId, termBackground, lang]);

  useEffect(() => {
    const el = termOutRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [termOutput]);

  const guardLocalWrite = (what: string): boolean => {
    if (settings.turboMode === "readonly") {
      setNotice(t(lang, "readonlyBlocked"));
      return false;
    }
    if (settings.turboMode === "ask") {
      return window.confirm(`${t(lang, "confirmLocalAction")}\n${what}`);
    }
    return true;
  };

  // C4 — lazy-expand: dirs deeper than the eager depth arrive with
  // children === null; fetch that subtree on first expand.
  const toggleDir = async (f: FileEntry) => {
    const willOpen = !openDirs[f.rel];
    setOpenDirs((prev) => ({ ...prev, [f.rel]: willOpen }));
    if (willOpen && f.children === null) {
      setLoadingDirs((prev) => ({ ...prev, [f.rel]: true }));
      try {
        const children = await invoke<FileEntry[]>("list_workspace_files", { root, rel: f.rel });
        setTree((prev) => updateTree(prev, f.rel, children));
      } catch (e) {
        setTreeError(String(e));
      } finally {
        setLoadingDirs((prev) => ({ ...prev, [f.rel]: false }));
      }
    }
  };

  // C8 — confirm before throwing away unsaved edits.
  const confirmDiscard = (): boolean => {
    if (!isDirty) return true;
    return window.confirm(t(lang, "editorDiscardConfirm"));
  };

  const loadFile = async (rel: string) => {
    setFileError(null);
    setFileChanged(false);
    try {
      const res = await invoke<{ rel: string; content: string; size: number }>("read_workspace_file", { root, rel });
      setOpenFile({ rel: res.rel, content: res.content });
      setEdited(res.content);
      setBaselineCrlf(res.content.includes("\r\n"));
    } catch (e) {
      const msg = String(e);
      // C6: the 2 MB Rust limit cannot be raised from the frontend —
      // say so plainly instead of showing the raw error.
      setFileError(msg.includes("too large") ? t(lang, "editorTooLarge") : msg);
    }
  };

  const openFileEntry = async (f: FileEntry) => {
    if (openFile && f.rel !== openFile.rel && !confirmDiscard()) return;
    await loadFile(f.rel);
  };

  const closeEditor = () => {
    if (!confirmDiscard()) return;
    setOpenFile(null);
    setEdited("");
    setFileError(null);
    setFileChanged(false);
  };

  const saveOpenFile = async () => {
    if (!openFile) return;
    if (!guardLocalWrite(`Save ${openFile.rel}`)) return;
    try {
      // C7 — concurrency check: re-read; refuse when the disk copy
      // changed since the file was opened (WinForms FileChangedOnDisk).
      const current = await invoke<{ rel: string; content: string; size: number }>("read_workspace_file", { root, rel: openFile.rel });
      if (current.content !== openFile.content) {
        setFileChanged(true);
        setFileError(t(lang, "fileChangedOnDisk"));
        return;
      }
      const content = toLineEndings(edited, baselineCrlf);
      await invoke("write_workspace_file", { root, rel: openFile.rel, content });
      props.onRecordChange({
        id: `chg-${Date.now()}`,
        rel: openFile.rel,
        kind: "write",
        createdAt: Date.now(),
        previousContent: openFile.content,
        by: "local-edit",
      });
      props.onNoteEditorChange(openFile.rel);
      setOpenFile({ ...openFile, content: edited });
      setFileChanged(false);
      setNotice(tf(lang, "fileSaved", openFile.rel));
      void refreshTree();
    } catch (e) {
      setFileError(String(e));
    }
  };

  const runTerminal = async () => {
    if (!termCmd.trim() || termRunning) return;
    if (!guardLocalWrite(`Run in terminal: ${termCmd.trim()}`)) return;
    setTermOutput("");
    setTermFinished(false);
    try {
      const id = await invoke<number>("start_terminal", {
        root,
        command: termCmd.trim(),
        shell: settings.shell || null,
      });
      setTermRunId(id);
      setTermRunning(true);
    } catch (e) {
      setTermOutput(String(e));
    }
  };

  const stopTerminal = async () => {
    if (termRunId === null) return;
    try {
      const out = await invoke<{ id: number; output: string; running: boolean }>("stop_terminal", { id: termRunId });
      setTermOutput(out.output);
    } catch (e) {
      setTermOutput(String(e));
    }
    setTermRunning(false);
    setTermFinished(true);
  };

  // D5 — send the finished run's output to the agent as context.
  const sendTerminalToAgent = () => {
    if (!termOutput.trim()) return;
    const head = `${t(lang, "terminalResultPrefix")} \`${termCmd.trim()}\`:\n\`\`\`\n`;
    props.onSendToAgent(`${head}${termOutput.slice(-4000)}\n\`\`\``);
  };

  const undoChange = async (c: FileChange) => {
    if (c.by === "agent") {
      // Server-side revert: the gateway restores the file from its journal.
      try {
        await props.onRevertAgentChange(c);
        setNotice(`Reverted ${c.rel}`);
        void refreshTree();
      } catch (e) {
        setNotice(String(e));
      }
      return;
    }
    if (c.previousContent === null) {
      setNotice(t(lang, "undoNotPossible"));
      return;
    }
    if (!guardLocalWrite(`Undo ${c.rel}`)) return;
    try {
      await invoke("write_workspace_file", { root, rel: c.rel, content: c.previousContent });
      props.onUndoChange(c);
      setNotice(`Restored ${c.rel}`);
      void refreshTree();
    } catch (e) {
      setNotice(String(e));
    }
  };

  const shellOptions = navigator.platform.toLowerCase().includes("win")
    ? ["cmd", "powershell", "pwsh"]
    : ["sh", "bash", "zsh"];

  return (
    <div className="flex h-full flex-col bg-[#1D1D26] text-[#F5F5F7]">
      {/* Header */}
      <div className="flex items-center gap-2 border-b border-[#343541] px-3 py-2.5">
        <span className="text-[12px] font-semibold uppercase tracking-wide text-[#A1A1AA]">
          {t(lang, "workspace")}
        </span>
        <span className="min-w-0 flex-1 truncate rounded bg-[#292A34] px-2 py-0.5 font-mono text-[11px] text-[#A1A1AA]" title={root || t(lang, "noWorkspace")}>
          {root ? root.split(/[\\/]/).filter(Boolean).pop() : "—"}
        </span>
        <span className="rounded border border-[#343541] px-1.5 py-0.5 text-[10px] text-[#A1A1AA]">{t(lang, "localBadge")}</span>
        <Button variant="ghost" size="icon" className="h-7 w-7 text-[#A1A1AA]" title={t(lang, "refresh")} aria-label={t(lang, "refresh")} onClick={() => void refreshTree()}>
          <RefreshCw className="h-3.5 w-3.5" />
        </Button>
        <Button variant="ghost" size="icon" className="h-7 w-7 text-[#A1A1AA]" title={t(lang, "collapse")} aria-label={t(lang, "collapse")} onClick={() => props.onCollapsedChange(true)}>
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>

      {notice && (
        <div className="flex items-center justify-between gap-2 border-b border-[#343541] bg-[#292A34] px-3 py-1.5 text-[11px] text-[#A1A1AA]">
          <span className="min-w-0 flex-1 truncate">{notice}</span>
          <button onClick={() => setNotice(null)} className="hover:text-white" aria-label={t(lang, "close")}><X className="h-3 w-3" /></button>
        </div>
      )}

      {!root ? (
        <div className="flex-1 overflow-y-auto p-4 text-[13px] leading-relaxed text-[#71717A]">
          {t(lang, "noWorkspace")}
        </div>
      ) : (
        <Tabs value={tab} onValueChange={(v) => onTabChange(v as WorkspaceTab)} className="flex min-h-0 flex-1 flex-col">
          <TabsList className="mx-2 mt-2 justify-start bg-[#292A34]">
            <TabsTrigger value="files" className="text-[12px]">{t(lang, "files")}</TabsTrigger>
            <TabsTrigger value="changes" className="text-[12px]">
              {t(lang, "changes")}{props.changes.length > 0 ? ` (${props.changes.length})` : ""}
            </TabsTrigger>
            <TabsTrigger value="terminal" className="text-[12px]">{t(lang, "terminal")}</TabsTrigger>
            <TabsTrigger value="screen" className="text-[12px]">
              {t(lang, "screen")}
              {screenTarget && <span className="ml-1 text-emerald-400" title={screenTarget.title}>●</span>}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="files" className="mt-0 min-h-0 flex-1 overflow-y-auto px-2 py-2">
            {treeLoading && <p className="px-2 text-[12px] text-[#71717A]">{t(lang, "loading")}</p>}
            {treeError && <p className="px-2 text-[12px] text-red-300">{treeError}</p>}
            <FileTree entries={tree} depth={0} openDirs={openDirs} loadingDirs={loadingDirs} onToggleDir={(f) => void toggleDir(f)} onOpen={(f) => void openFileEntry(f)} />
            {openFile && (
              <div className="mt-3 overflow-hidden rounded-lg border border-[#343541]">
                <div className="flex items-center gap-2 bg-[#292A34] px-2.5 py-2">
                  <FileText className="h-4 w-4 shrink-0 text-[#A1A1AA]" />
                  <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-[#A1A1AA]" title={openFile.rel}>
                    {openFile.rel}{isDirty ? t(lang, "editorUnsaved") : ""}
                  </span>
                  <Button
                    size="sm"
                    variant="outline"
                    className="h-7 shrink-0 border-[#343541] text-[#A1A1AA]"
                    title={t(lang, "askMuse")}
                    onClick={() => props.onAskMuse(tf(lang, "askMuseFilePrompt", openFile.rel))}
                  >
                    <MessageSquarePlus className="h-3.5 w-3.5" /> {t(lang, "askMuse")}
                  </Button>
                  <Button size="sm" className="h-7 bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={() => void saveOpenFile()} title={`${t(lang, "save")} (Ctrl+S)`}>
                    <Save className="h-3.5 w-3.5" /> {t(lang, "save")}
                  </Button>
                  <Button variant="ghost" size="icon" className="h-7 w-7" onClick={closeEditor} title={t(lang, "close")} aria-label={t(lang, "close")}>
                    <X className="h-3.5 w-3.5" />
                  </Button>
                </div>
                {fileError && (
                  <div className="flex items-center gap-2 bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-300">
                    <span className="min-w-0 flex-1">{fileError}</span>
                    {fileChanged && (
                      <Button size="sm" variant="outline" className="h-6 shrink-0 border-red-500/40 text-red-200" onClick={() => void loadFile(openFile.rel)}>
                        {t(lang, "fileReload")}
                      </Button>
                    )}
                  </div>
                )}
                <CodeEditor
                  value={edited}
                  onChange={setEdited}
                  fileRel={openFile.rel}
                  onSave={() => void saveOpenFile()}
                />
              </div>
            )}
          </TabsContent>

          <TabsContent value="changes" className="mt-0 min-h-0 flex-1 overflow-y-auto px-2 py-2">
            {props.changes.length === 0 ? (
              <p className="px-2 py-3 text-[12px] text-[#71717A]">{t(lang, "noChanges")}</p>
            ) : (
              <ul className="space-y-1.5">
                {props.changes.map((c) => (
                  <li key={c.id} className="rounded-md border border-[#343541] bg-[#292A34] px-2.5 py-2">
                    <div className="flex items-center gap-2">
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-mono text-[12px] text-[#F5F5F7]">{c.rel}</span>
                        <span className="text-[11px] text-[#71717A]">
                          {new Date(c.createdAt).toLocaleString()} · {c.by === "local-edit" ? t(lang, "changeByLocalEdit") : c.by === "agent" ? t(lang, "changeByAgent") : t(lang, "changeByTerminal")}
                        </span>
                      </span>
                      {c.diff && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-7 shrink-0 text-[#A1A1AA]"
                          onClick={() => setOpenDiffs((prev) => ({ ...prev, [c.id]: !prev[c.id] }))}
                          aria-expanded={!!openDiffs[c.id]}
                        >
                          {openDiffs[c.id] ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                          {t(lang, "changesDiff")}
                        </Button>
                      )}
                      <Button variant="outline" size="sm" className="h-7 shrink-0 border-[#343541] text-[#A1A1AA]" onClick={() => void undoChange(c)}>
                        <Undo2 className="h-3.5 w-3.5" /> {t(lang, "undo")}
                      </Button>
                    </div>
                    {c.diff && openDiffs[c.id] && (
                      <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap rounded border border-[#343541] bg-[#17171D] p-2 font-mono text-[11px] leading-relaxed text-[#A1A1AA]">
                        {c.diff.slice(0, 20000)}
                      </pre>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </TabsContent>

          <TabsContent value="terminal" className="mt-0 flex min-h-0 flex-1 flex-col px-2 py-2">
            <div className="mb-2 flex gap-1.5">
              <select
                value={settings.shell || shellOptions[0]}
                onChange={(e) => props.onShellChange(e.currentTarget.value)}
                className="rounded-md border border-[#343541] bg-[#292A34] px-2 py-1.5 text-[12px] text-[#F5F5F7]"
                title="Shell for terminal commands (cwd = workspace)"
              >
                {shellOptions.map((s) => (
                  <option key={s} value={s}>{s}</option>
                ))}
              </select>
              <Input
                value={termCmd}
                onChange={(e) => setTermCmd(e.currentTarget.value)}
                onKeyDown={(e) => { if (e.key === "Enter") void runTerminal(); }}
                placeholder={t(lang, "commandPlaceholder")}
                className="border-[#343541] bg-[#292A34] font-mono text-[12px] text-[#F5F5F7] placeholder:text-[#71717A]"
                aria-label={t(lang, "commandPlaceholder")}
              />
              {termRunning ? (
                <Button size="sm" variant="destructive" onClick={() => void stopTerminal()} title={t(lang, "stop")}>
                  <Square className="h-3.5 w-3.5" /> {t(lang, "stop")}
                </Button>
              ) : (
                <Button size="sm" className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={() => void runTerminal()}>
                  <Play className="h-3.5 w-3.5" /> {t(lang, "run")}
                </Button>
              )}
            </div>
            <label className="mb-2 flex cursor-pointer items-center gap-2 text-[12px] text-[#A1A1AA]">
              <input
                type="checkbox"
                checked={termBackground}
                onChange={(e) => setTermBackground(e.currentTarget.checked)}
                className="accent-[#3867F5]"
              />
              {t(lang, "runInBackground")}
            </label>
            <div ref={termOutRef} className="min-h-0 flex-1 overflow-y-auto rounded-lg border border-[#343541] bg-[#17171D] p-2.5">
              <pre className="whitespace-pre-wrap font-mono text-[11.5px] leading-relaxed text-[#A1A1AA]">
                {termOutput ? termOutput.slice(-MAX_DISPLAY_OUTPUT) : "$ " + (termCmd || "—")}
              </pre>
            </div>
            {termFinished && termOutput.trim() && (
              <Button
                size="sm"
                variant="outline"
                className="mt-2 self-start border-[#343541] text-[#A1A1AA]"
                onClick={sendTerminalToAgent}
              >
                <Send className="h-3.5 w-3.5" /> {t(lang, "sendResultToMuse")}
              </Button>
            )}
            <p className="mt-1.5 text-[11px] text-[#71717A]">
              {t(lang, "localBadge")} · cwd = {root} · {settings.shell || shellOptions[0]}
            </p>
          </TabsContent>

          <TabsContent value="screen" className="mt-0 min-h-0 flex-1 overflow-y-auto px-3 py-3">
            <div className="space-y-2.5">
              {/* Header: live badge + authorized target */}
              <div className="flex items-center gap-2">
                <Monitor className="h-4 w-4 shrink-0 text-[#A1A1AA]" />
                <span className="text-[13px] font-semibold">{t(lang, "screen")}</span>
                {screenStatus && (
                  <span className="rounded bg-emerald-500/20 px-1.5 py-0.5 text-[10px] font-semibold text-emerald-300">
                    {t(lang, "liveBadge")}
                  </span>
                )}
                {screenTarget && (
                  <span className="min-w-0 flex-1 truncate text-[12px] text-emerald-300" title={screenTarget.title}>
                    ● {screenTarget.title || t(lang, "screenUntitled")}
                  </span>
                )}
              </div>

              {screenLoading && <p className="text-[12px] text-[#71717A]">{t(lang, "loading")}</p>}
              {screenError && (
                <p className="text-[12px] leading-relaxed text-red-300">
                  {t(lang, "screenUnreachable")} ({screenError.slice(0, 200)})
                </p>
              )}
              {screenStatus && !screenStatus.available && (
                <p className="text-[12px] leading-relaxed text-[#A1A1AA]">
                  {screenStatus.reason || t(lang, "screenUnavailable")}
                </p>
              )}
              {screenReadonly && (
                <p className="text-[12px] leading-relaxed text-amber-300">{t(lang, "screenReadonlyNote")}</p>
              )}
              {screenActionError && (
                <p className="text-[12px] leading-relaxed text-red-300">{screenActionError.slice(0, 300)}</p>
              )}

              {/* Search + refresh (WinForms ComputerScopeForm) */}
              <div className="flex gap-1.5">
                <Input
                  value={screenSearch}
                  onChange={(e) => setScreenSearch(e.currentTarget.value)}
                  placeholder={t(lang, "screenSearch")}
                  className="border-[#343541] bg-[#292A34] text-[12px] text-[#F5F5F7] placeholder:text-[#71717A]"
                  aria-label={t(lang, "screenSearch")}
                />
                <Button
                  variant="secondary"
                  size="sm"
                  className="shrink-0 bg-[#292A34] text-[#F5F5F7] hover:bg-[#343541]"
                  onClick={() => void loadScreen()}
                  title={t(lang, "refresh")}
                >
                  <RefreshCw className="h-3.5 w-3.5" /> {t(lang, "refresh")}
                </Button>
              </div>

              {/* Window list */}
              <p className="text-[11px] text-[#71717A]">
                {tf(lang, "screenWindowCount", filteredWindows.length)}
                {screenWindowsLoading ? ` · ${t(lang, "loading")}` : ""}
              </p>
              <div className="max-h-56 space-y-1 overflow-y-auto">
                {!screenWindowsLoading && filteredWindows.length === 0 && (
                  <p className="py-2 text-[12px] text-[#71717A]">{t(lang, "screenNoWindows")}</p>
                )}
                {filteredWindows.map((w) => {
                  const isTarget = screenTarget !== null && screenTarget.handle === w.handle;
                  const isSelected = screenSelected !== null && screenSelected.handle === w.handle;
                  return (
                    <button
                      key={w.handle}
                      onClick={() => setScreenSelected(w)}
                      onDoubleClick={() => void authorizeWindow(w)}
                      className={`flex w-full items-center gap-2 rounded-md border px-2.5 py-2 text-left ${
                        isSelected
                          ? "border-[#3867F5] bg-[#3867F5]/15"
                          : "border-[#343541] bg-[#292A34] hover:border-[#4a4b58]"
                      }`}
                      title={`${w.title || t(lang, "screenUntitled")} — ${w.application_name ?? ""} (PID ${w.process_id})`}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[12.5px] text-[#F5F5F7]">
                          {w.title || t(lang, "screenUntitled")}
                        </span>
                        <span className="block truncate text-[11px] text-[#71717A]">
                          {w.application_name ? `${w.application_name} · ` : ""}PID {w.process_id}
                        </span>
                      </span>
                      {isTarget && (
                        <span className="shrink-0 text-[11px] font-semibold text-emerald-400" title={t(lang, "screenAuthorized")}>
                          ● {t(lang, "screenAuthorized")}
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
              <p className="text-[11px] leading-relaxed text-[#71717A]">{t(lang, "screenSelectHint")}</p>

              {/* Actions */}
              <div className="flex flex-wrap gap-1.5">
                <Button
                  size="sm"
                  className="bg-[#3867F5] text-white hover:bg-[#2f5ae0] disabled:opacity-50"
                  disabled={!screenSelected || screenBusy || screenReadonly || !screenStatus?.available}
                  onClick={() => screenSelected && void authorizeWindow(screenSelected)}
                >
                  {t(lang, "screenAllow")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="border-[#343541] text-[#A1A1AA] disabled:opacity-50"
                  disabled={screenBusy || screenReadonly || !screenStatus?.available}
                  onClick={() => void autoAuthorizeScreen()}
                >
                  {t(lang, "screenAutoSelect")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  className="border-[#343541] text-[#A1A1AA] disabled:opacity-50"
                  disabled={!screenTarget || screenBusy}
                  onClick={() => void disarmScreen()}
                >
                  {t(lang, "screenDisarm")}
                </Button>
              </div>

              {/* Live preview (max 1 poll per 1.5s while on + target set) */}
              <label className="flex cursor-pointer items-center gap-2 text-[12px] text-[#A1A1AA]">
                <input
                  type="checkbox"
                  checked={livePreview}
                  onChange={(e) => setLivePreview(e.currentTarget.checked)}
                  className="accent-[#3867F5]"
                />
                {t(lang, "screenLivePreview")}
              </label>
              <div className="overflow-hidden rounded-lg border border-[#343541] bg-[#17171D]">
                {previewUrl ? (
                  <img
                    src={previewUrl}
                    alt={screenTarget?.title || t(lang, "screenLivePreview")}
                    className="w-full"
                  />
                ) : (
                  <p className="p-3 text-[12px] text-[#71717A]">
                    {!screenTarget
                      ? t(lang, "screenNoTarget")
                      : previewError
                        ? `${t(lang, "screenPreviewUnavailable")} (${previewError.slice(0, 200)})`
                        : !livePreview
                          ? t(lang, "screenLivePreview")
                          : t(lang, "loading")}
                  </p>
                )}
              </div>
            </div>
          </TabsContent>
        </Tabs>
      )}
    </div>
  );
}
