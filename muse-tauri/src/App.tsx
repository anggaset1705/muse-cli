// Muse — Tauri v2 three-panel app.
//
// Layout: sidebar (260px) | chat (flex) | workspace (305px), resizable via
// shadcn Resizable (react-resizable-panels). Colours: bg #17171D,
// sidebar/workspace #1D1D26, chat #22232C, secondary #292A34,
// border #343541, accent #3867F5, text #F5F5F7 / #A1A1AA / #71717A.
//
// Wiring honesty: chat completions, models, health and admin calls go to
// the Rust muse-gateway over HTTP, and the agent loop is driven through
// /v1/agent/* (one server session per conversation, SSE turns with
// approval). Conversation history, workspace files, terminal, changes
// and skills are LOCAL to this app. Screen control runs through the
// gateway's /v1/agent/screen/* endpoints (UIA on Windows).
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { X } from "lucide-react";
import { PanelRightOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable";
import { Sidebar } from "@/components/Sidebar";
import { ChatPanel } from "@/components/ChatPanel";
import { WorkspacePanel, type WorkspaceTab } from "@/components/WorkspacePanel";
import { SettingsDialog } from "@/components/SettingsDialog";
import { LoginDialog } from "@/components/LoginDialog";
import type {
  AgentTurnEvent,
  Attachment,
  BackendStatus,
  ChatMessage,
  Conversation,
  FileChange,
  GatewayAccount,
  PendingApproval,
  Settings,
  ToolActionState,
  ToolPlanAction,
} from "@/lib/types";
import { newConversation, newMessageId } from "@/lib/types";
import { detectReplyLanguage, t } from "@/lib/i18n";
import { streamingReplyDisplay, unwrapReplyEnvelope } from "@/lib/replyEnvelope";
import { friendlyError } from "@/lib/friendlyErrors";
import { activityLine } from "@/lib/toolSummaries";
import { checkForUpdates, dismissUpdate, getDismissedVersion, isTrustedUpdateLink, type AvailableUpdate } from "@/lib/updates";
import { DialogPluginMissingError, pickFolder } from "@/lib/dialog";
import {
  loadChanges,
  loadConversations,
  loadSettings,
  saveChanges,
  saveConversations,
  saveSettings,
} from "@/lib/store";
import { normalizeWorkspacePath, sameWorkspacePath } from "@/lib/workspacePath";
import {
  anySavedLogin,
  approveAgentTool,
  cancelAgentTurn,
  createAgentSession,
  deleteAgentSession,
  fetchAccounts,
  fetchModels,
  gatewayHealth,
  getAgentChanges,
  revertAgentChange as apiRevertAgentChange,
  streamAgentTurn,
  streamChatCompletion,
  unwrapData,
  updateAgentPreferences,
} from "@/lib/gateway";
import "./App.css";

const emptyBackend: BackendStatus = {
  running: false,
  healthy: false,
  base_url: null,
  port: null,
  pid: null,
  launch_mode: null,
  error: null,
};

/** The picker shows a single model, "Muse" (muse-chat). The gateway
 *  still serves muse-image/muse-video via /v1/models (left untouched
 *  for OpenAI compatibility), but Bree asked for one model only. */
const REAL_MODELS = ["muse-chat"];

function loadAgentSessions(): Record<string, { id: string; workspace: string }> {
  try {
    return JSON.parse(localStorage.getItem("muse-agent-sessions-v1") ?? "{}") as Record<string, { id: string; workspace: string }>;
  } catch {
    return {};
  }
}

function download(filename: string, text: string, mime: string) {
  const blob = new Blob([text], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function detectToolPlan(text: string): ToolPlanAction[] | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{")) return null;
  try {
    const parsed = JSON.parse(trimmed) as { actions?: unknown };
    if (Array.isArray(parsed.actions)) return parsed.actions as ToolPlanAction[];
    return null;
  } catch {
    return null;
  }
}

function App() {
  const [settings, setSettings] = useState<Settings>(() => loadSettings());
  const [conversations, setConversations] = useState<Conversation[]>(() => loadConversations(loadSettings().workspacePath));
  const [activeId, setActiveId] = useState<string | null>(() => {
    const list = loadConversations(loadSettings().workspacePath);
    return list.length > 0 ? list[0].id : null;
  });
  const [changes, setChanges] = useState<FileChange[]>(() => loadChanges());
  const [backend, setBackend] = useState<BackendStatus>(emptyBackend);
  const [backendError, setBackendError] = useState<string | null>(null);
  // True while the backend is (auto-)starting and has not yet failed:
  // the ChatPanel shows "starting…" instead of the "Gateway unreachable"
  // banner + Start button until the backend is healthy or the start has
  // really failed. Starts true — the app auto-starts on mount.
  const [backendStarting, setBackendStarting] = useState(true);
  const [gatewayOk, setGatewayOk] = useState(false);
  // Account list (same source as SettingsDialog: GET /admin/accounts) —
  // drives the welcome "Connected" pill AND the send gate. Kept in App
  // (not a child) so every consumer sees the same list. A failed load
  // never claims connected: accountsLoaded stays false until a load
  // succeeds, and a failed refresh keeps the previous list.
  const [accounts, setAccounts] = useState<GatewayAccount[]>([]);
  const [accountsLoaded, setAccountsLoaded] = useState(false);
  /** Real connection state (hasSavedLogin rule): only true from a
   * successfully loaded list with ≥1 saved login. Backend health alone
   * never makes this true — a healthy backend with no account shows
   * "Not connected". */
  const accountConnected = accountsLoaded && anySavedLogin(accounts);
  const [models, setModels] = useState<string[]>(["muse-chat"]);
  const [streaming, setStreaming] = useState(false);
  const [streamingText, setStreamingText] = useState("");
  const [streamingPlan, setStreamingPlan] = useState<ToolPlanAction[] | null>(null);
  const [streamingActions, setStreamingActions] = useState<ToolActionState[] | null>(null);
  const [streamingThoughts, setStreamingThoughts] = useState<string[]>([]);
  const [goalActive, setGoalActive] = useState(false);
  const [pendingApproval, setPendingApproval] = useState<PendingApproval | null>(null);
  const [filesVersion, setFilesVersion] = useState(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);
  /** How LoginDialog should open: plain restore (default) or "add account"
   * (Settings: cleared WebView session + prefilled "Account N" label).
   * Set together with loginOpen so the dialog sees both at once.
   * `notice` is the send-gate banner (App.sendMessage): shown when the
   * dialog was opened because the user tried to chat with no account. */
  const [loginRequest, setLoginRequest] = useState<{ clearSession: boolean; label?: string; notice?: string } | null>(null);
  /** Sequence bumped on every openLogin() call: LoginDialog is keyed by
   * it (see the `key={loginSeq}` below), so each opening is a FRESH
   * mount — no label / mode / error state from a previous opening can
   * ever leak into the next one, even if the dialog was already open
   * when openLogin was called again. */
  const [loginSeq, setLoginSeq] = useState(0);
  const openLogin = (req?: { clearSession: boolean; label?: string; notice?: string }) => {
    setLoginRequest(req ?? null);
    setLoginSeq((s) => s + 1);
    setLoginOpen(true);
  };
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [workspaceTab, setWorkspaceTab] = useState<WorkspaceTab>("files");
  const [failedDraft, setFailedDraft] = useState<{ text: string; at: number } | null>(null);
  const [composerPrefill, setComposerPrefill] = useState<{ text: string; at: number } | null>(null);
  const [availableUpdate, setAvailableUpdate] = useState<AvailableUpdate | null>(null);

  const lang = settings.uiLanguage;
  const active = conversations.find((c) => c.id === activeId) ?? null;
  const effectiveBase = backend.base_url ?? settings.backendBaseUrl;
  const backendHealthy = backend.healthy || gatewayOk;
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const backendRef = useRef(backend);
  backendRef.current = backend;
  const convsRef = useRef(conversations);
  convsRef.current = conversations;
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;
  const streamingRef = useRef(streaming);
  streamingRef.current = streaming;
  const streamingTextRef = useRef(streamingText);
  streamingTextRef.current = streamingText;
  // What the typewriter has actually typed so far (mirrored by
  // ChatPanel on every tick). Stop saves THIS into the message, not the
  // full streamed text — otherwise the saved message would jump ahead of
  // what the user saw typed. Falls back to streamingText when nothing
  // was typed yet (Stop pressed within ~16 ms of the first delta).
  const streamingDisplayedRef = useRef("");
  const streamingThoughtsRef = useRef(streamingThoughts);
  streamingThoughtsRef.current = streamingThoughts;
  const settingsOpenRef = useRef(settingsOpen);
  settingsOpenRef.current = settingsOpen;
  const loginOpenRef = useRef(loginOpen);
  loginOpenRef.current = loginOpen;

  // Per-turn token: stopTurn() bumps it so a late SSE event / rejection
  // from the cancelled turn can never overwrite the stopped message.
  const turnTokenRef = useRef(0);
  const turnStartedAtRef = useRef(0);
  const turnAssistantIdRef = useRef<string | null>(null);
  const turnConvIdRef = useRef<string | null>(null);
  // C13 — editor saves waiting to be prepended to the next agent prompt.
  const editorContextRef = useRef<string[]>([]);
  // C8 — the editor's dirty state, mirrored to Rust (set_editor_dirty) so
  // the Rust close interceptor can block the window close while dirty.
  const editorDirtyRef = useRef(false);

  // C8 — Rust blocked the window close because the editor is dirty
  // ("app-close-requested" from on_window_event): ask (localized) and
  // only then let Rust destroy the window. If the user cancels, nothing
  // happens — the window simply stays open.
  useEffect(() => {
    const promise = listen("app-close-requested", () => {
      if (window.confirm(t(settingsRef.current.uiLanguage, "editorDiscardConfirm"))) {
        void invoke("confirm_app_close").catch(() => {
          /* window already gone or close already allowed — ignore */
        });
      }
    });
    return () => {
      void promise.then((unlisten) => unlisten());
    };
  }, []);

  const reportEditorDirty = useCallback((dirty: boolean) => {
    editorDirtyRef.current = dirty;
    void invoke("set_editor_dirty", { dirty }).catch(() => {
      /* Rust close guard unavailable (dev web build) — close just proceeds */
    });
  }, []);

  // B6 — the workspace the loaded conversation list belongs to. The
  // list is reloaded (and the old one saved under its own key) whenever
  // the workspace path changes. Compared NORMALISED (defect B): the
  // verbatim form `\\?\C:\FOX` and the plain `C:\FOX` are the same
  // workspace, so a resolve that only changes the spelling must NOT
  // swap the conversation list out from under the user.
  const convWsRef = useRef(settings.workspacePath);
  useEffect(() => {
    const next = normalizeWorkspacePath(settings.workspacePath);
    if (sameWorkspacePath(next, convWsRef.current)) {
      // Spelling-only change (e.g. a stale `\\?\` in stored settings):
      // keep the same list, just record the normalised spelling.
      if (next !== convWsRef.current) convWsRef.current = next;
      return;
    }
    saveConversations(convsRef.current, convWsRef.current);
    convWsRef.current = next;
    const list = loadConversations(next);
    setConversations(list);
    setActiveId(list.length > 0 ? list[0].id : null);
  }, [settings.workspacePath]);

  // Agent sessions: one server session per conversation, persisted so a
  // reload reuses the same loop (and its history). { convId: { id, workspace } }
  const agentSessionsRef = useRef<Record<string, { id: string; workspace: string }>>(loadAgentSessions());
  const saveAgentSessions = () => {
    try {
      localStorage.setItem("muse-agent-sessions-v1", JSON.stringify(agentSessionsRef.current));
    } catch {
      /* storage unavailable — sessions just won't survive reload */
    }
  };
  const pendingApprovalRef = useRef<PendingApproval | null>(null);
  pendingApprovalRef.current = pendingApproval;

  // I2 — Light theme: applied on <html> so Radix portals (dialogs)
  // outside the App root get themed too. Palette in App.css.
  useEffect(() => {
    document.documentElement.classList.toggle("theme-light", settings.theme === "light");
  }, [settings.theme]);

  // I13 — update check on launch + every 6h (lib/updates caches).
  useEffect(() => {
    let cancelled = false;
    const check = async () => {
      const res = await checkForUpdates(false);
      if (cancelled) return;
      if (res.status === "available" && res.update && isTrustedUpdateLink(res.update)) {
        if (getDismissedVersion() !== res.update.version) setAvailableUpdate(res.update);
      }
    };
    void check();
    const id = window.setInterval(() => void check(), 6 * 60 * 60 * 1000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  // --- backend status polling ---
  // Overlap guard: gatewayHealth can take up to ~30s (/readyz with the
  // browser driver), far longer than the 5s tick. Without this guard a
  // hanging poll stacks up requests and can fill the browser's
  // 6-connections-per-origin pool, stalling every gateway call (and
  // making the app look frozen / "Not Responding").
  const refreshInFlightRef = useRef(false);
  const refreshBackend = useCallback(async () => {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    try {
      const s = await invoke<BackendStatus>("backend_status");
      setBackend(s);
      // Persist the sidecar's ACTUAL listening URL: every restart picks
      // a fresh random port, and the stored default (18610) is stale
      // after the first launch. Without this, effectiveBase falls back
      // to the dead port whenever backend.base_url is momentarily null
      // (backend down), so the next account/usage fetch targets a dead
      // port. Only the live URL is ever stored — never a guess.
      if (s.base_url && s.base_url !== settingsRef.current.backendBaseUrl) {
        const liveBase = s.base_url;
        setSettings((prev) => ({ ...prev, backendBaseUrl: liveBase }));
      }
      // Surface the last failure reason reported by Rust, and clear the
      // error once the backend is healthy again.
      if (s.error && !s.running) setBackendError(s.error);
      if (s.healthy) setBackendError(null);
      // Starting is over once the backend is healthy, or once it has died
      // with a recorded reason. Merely "not running yet" does NOT clear
      // it — the auto-start invoke may still be in flight.
      if (s.healthy || (s.error && !s.running)) setBackendStarting(false);
      const base = s.base_url ?? settingsRef.current.backendBaseUrl;
      const h = await gatewayHealth(base);
      setGatewayOk(h.ok || h.ready);
      if (h.ok) {
        // Pick up the sidecar's bearer key (generated by start_backend and
        // passed via env). Without it every /v1/agent/* call goes out with
        // no Authorization header and the gateway answers 401. When the
        // backend was NOT started by this app the command errors and the
        // key typed in Settings keeps applying.
        try {
          const key = await invoke<string>("backend_api_key");
          if (key && key !== settingsRef.current.apiKey) {
            setSettings((prev) => ({ ...prev, apiKey: key }));
          }
        } catch {
          /* external/manual gateway — keep the Settings key */
        }
        try {
          const m = await fetchModels(base, settingsRef.current.apiKey);
          // Picker shows a single model ("Muse" = muse-chat);
          // fall back to muse-chat when it is not listed.
          const listed = REAL_MODELS.filter((id) => m.includes(id));
          const filtered = listed.length > 0 ? listed : ["muse-chat"];
          setModels(filtered);
          if (!filtered.includes(settingsRef.current.model)) {
            // A stored model that is not muse-chat (e.g. muse-image,
            // muse-video, or an alias) resets to muse-chat.
            setSettings((prev) => ({ ...prev, model: "muse-chat" }));
          }
        } catch {
          /* models fetch failed — keep fallback list */
        }
      }
    } catch {
      setBackend(emptyBackend);
      setGatewayOk(false);
    } finally {
      refreshInFlightRef.current = false;
    }
  }, []);

  useEffect(() => {
    void refreshBackend();
    const t = window.setInterval(() => {
      void refreshBackend();
    }, 5000);
    return () => window.clearInterval(t);
  }, [refreshBackend]);

  // Defect B — a stale backend error (e.g. from a failed auto-start
  // during install/swap, or a folder-picker / import failure surfaced
  // through the same state) must never outlive a healthy backend. The
  // 5 s poll already clears it on `s.healthy`; this also covers the
  // gateway-healthy case and any error set between polls, so Settings /
  // Sidebar never show a dead failure next to a green backend.
  useEffect(() => {
    if (backendHealthy) setBackendError(null);
  }, [backendHealthy]);
  // Called when the backend becomes healthy and whenever an account may
  // have changed: Settings/LoginDialog close (add / delete / toggle there)
  // and the send gate's own check. NOT on the 5 s backend poll — the
  // list only changes via those user actions, plus the key rotation
  // below (refreshAccounts' identity changes when settings.apiKey does,
  // which re-runs the loader effect; the poll itself never calls this).
  //
  // BUG (9 Nov 2026): the loader effect used to fire only on the
  // false→true flip of backendHealthy, at which point settings.apiKey
  // still held the PREVIOUS launch's key — the fresh key is only picked
  // up later in refreshBackend, after gatewayHealth resolves. That
  // first fetch 401'd, the error was swallowed, and nothing ever
  // re-fetched, so the pill stayed "Not connected" until a manual
  // dialog close. Fix: refreshAccounts now closes over settings.apiKey
  // (state, not the ref), so a key rotation re-creates it and the
  // loader effect below re-runs with the fresh key.
  //
  // In-flight guard: at most one /admin/accounts fetch runs at a time.
  // If a call arrives while one is in flight (e.g. the stale-key fetch
  // is still running when the key rotates), the LATEST key is remembered
  // and fetched once the in-flight one lands — a same-key duplicate is
  // dropped, so dialog closes can never double-fetch.
  const accountsInFlightRef = useRef(false);
  const accountsPendingKeyRef = useRef<string | null>(null);
  const fetchAccountsOnce = useCallback(
    async (key: string) => {
      if (accountsInFlightRef.current) {
        accountsPendingKeyRef.current = key;
        return;
      }
      accountsInFlightRef.current = true;
      try {
        const res = await fetchAccounts(effectiveBase, key);
        setAccounts((unwrapData<GatewayAccount[]>(res) ?? []) as GatewayAccount[]);
        setAccountsLoaded(true);
      } catch {
        /* keep the previous list; a failed load never claims connected */
      } finally {
        accountsInFlightRef.current = false;
        const pending = accountsPendingKeyRef.current;
        accountsPendingKeyRef.current = null;
        if (pending && pending !== key) void fetchAccountsOnce(pending);
      }
    },
    [effectiveBase],
  );
  const refreshAccounts = useCallback(async () => {
    const key = settings.apiKey;
    // Fail-open: no key yet (fresh install / external gateway without
    // one) means no fetch — an unauthenticated probe must never be
    // mistaken for a connection state. The send gate still does its own
    // fresh check at send time, and dialog closes re-trigger this once
    // a key exists.
    if (!key) return;
    await fetchAccountsOnce(key);
  }, [settings.apiKey, fetchAccountsOnce]);

  // Load the account list once the backend is healthy, so the welcome
  // pill reflects the real connection state from the first paint after
  // startup. backendHealthy is a boolean, so the 5 s backend poll does
  // not re-trigger this while it stays true — but a settings.apiKey
  // rotation DOES (via refreshAccounts' identity), which is exactly
  // what heals the stale-key first fetch after every restart.
  useEffect(() => {
    if (!backendHealthy) return;
    void refreshAccounts();
  }, [backendHealthy, refreshAccounts]);

  // Auto-start the backend on launch, like the WinForms muse.exe: no
  // manual "Start backend" click needed. Guarded by a ref so StrictMode's
  // dev double-effect (and any re-mount) never spawns it twice. A failure
  // is shown inline (backendError) instead of a blocking alert; the manual
  // Start button in the sidebar / login dialog stays as a fallback.
  const autoStartRef = useRef(false);
  useEffect(() => {
    if (autoStartRef.current) return;
    autoStartRef.current = true;
    if (backendRef.current.running) return;
    setBackendStarting(true);
    void (async () => {
      try {
        const s = await invoke<BackendStatus>("start_backend", {
          backendPath: null,
          driver: settingsRef.current.driver,
        });
        setBackend(s);
        setBackendError(null);
      } catch (e) {
        setBackendError(String(e));
        setBackendStarting(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Resolve the workspace folder once at startup: an empty setting, or a
  // stale Unix-style "/home/user/workspace" left in settings on Windows,
  // becomes a real folder ("<home>/Muse Workspace", created if missing).
  // The same resolution runs again before the first agent turn.
  useEffect(() => {
    void (async () => {
      try {
        const resolved = await invoke<string>("resolve_workspace", {
          requested: settingsRef.current.workspacePath || null,
        });
        // Defect B: only adopt the resolved path if it is a DIFFERENT
        // workspace — a spelling-only change (`\\?\` prefix, drive
        // case, separators) must not rewrite settings and must not
        // trip the conversation-list swap.
        const normalised = normalizeWorkspacePath(resolved);
        if (normalised && !sameWorkspacePath(normalised, settingsRef.current.workspacePath)) {
          setSettings((prev) => ({ ...prev, workspacePath: normalised }));
        }
      } catch {
        /* leave as-is; resolved again before the first agent turn */
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // --- persistence ---
  useEffect(() => saveConversations(conversations, convWsRef.current), [conversations]);
  useEffect(() => saveSettings(settings), [settings]);
  useEffect(() => saveChanges(changes), [changes]);

  // History-recovery (4 Nov 2026): if the startup load spuriously came
  // back empty while a non-empty list is stored for this workspace
  // (storage not ready at first read), restore the visible list instead
  // of showing an empty sidebar. The store-level guard already stops the
  // [] from overwriting storage; this only repairs the in-memory state.
  // Best-effort, and a no-op in every normal flow (a new workspace, or a
  // list the user explicitly cleared, both load [] here too).
  const historyRecoveredRef = useRef(false);
  useEffect(() => {
    if (historyRecoveredRef.current) return;
    historyRecoveredRef.current = true;
    if (convsRef.current.length === 0) {
      const stored = loadConversations(settingsRef.current.workspacePath);
      if (stored.length > 0) {
        setConversations(stored);
        if (!activeIdRef.current) setActiveId(stored[0].id);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // --- Ctrl/Cmd+K: new conversation ---
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        createConversation();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const startBackend = useCallback(async () => {
    setBackendStarting(true);
    try {
      const s = await invoke<BackendStatus>("start_backend", {
        backendPath: null,
        driver: settingsRef.current.driver,
      });
      setBackend(s);
      setBackendError(null);
    } catch (e) {
      setBackendError(String(e));
      setBackendStarting(false);
    }
  }, []);

  const stopBackend = useCallback(async () => {
    try {
      const s = await invoke<BackendStatus>("stop_backend");
      setBackend(s);
      setGatewayOk(false);
      setBackendError(null);
      setBackendStarting(false);
    } catch (e) {
      setBackendError(String(e));
    }
  }, []);

  const createConversation = useCallback(() => {
    const c = newConversation();
    c.model = settingsRef.current.model;
    setConversations((prev) => [c, ...prev]);
    setActiveId(c.id);
    return c;
  }, []);

  const updateConversation = useCallback((id: string, fn: (c: Conversation) => Conversation) => {
    setConversations((prev) => prev.map((c) => (c.id === id ? fn(c) : c)));
  }, []);

  // B7 — selecting a conversation restores the model it was used with.
  // The picker is single-model ("Muse" = muse-chat), so an old
  // conversation stored with muse-image/muse-video must not bring
  // that model back.
  const selectConversation = useCallback((id: string) => {
    setActiveId(id);
    const conv = convsRef.current.find((c) => c.id === id);
    if (conv?.model && conv.model !== settingsRef.current.model) {
      setSettings((s) => ({ ...s, model: "muse-chat" }));
    }
  }, []);

  // A12 — Stop the running turn (button + Esc). Cancels server-side,
  // marks the assistant message cancelled, and invalidates the turn so
  // late SSE events cannot overwrite it.
  const stopTurn = useCallback(async () => {
    if (!streamingRef.current) return;
    ++turnTokenRef.current; // invalidate the running turn's finish()
    const convId = turnConvIdRef.current;
    const assistantId = turnAssistantIdRef.current;
    const session = convId ? agentSessionsRef.current[convId] : undefined;
    setStreaming(false);
    setStreamingText("");
    setStreamingPlan(null);
    setStreamingActions(null);
    setPendingApproval(null);
    setGoalActive(false);
    // Save what was actually typed so far (see streamingDisplayedRef) —
    // the full streamed text may be ahead of the typewriter animation.
    const text = streamingDisplayedRef.current || streamingTextRef.current;
    const thoughts = [...streamingThoughtsRef.current];
    const seconds = turnStartedAtRef.current > 0 ? (Date.now() - turnStartedAtRef.current) / 1000 : undefined;
    if (convId && assistantId) {
      updateConversation(convId, (c) => ({
        ...c,
        updatedAt: Date.now(),
        messages: c.messages.map((m) =>
          m.id === assistantId
            ? {
                ...m,
                content: text,
                turnStatus: "cancelled",
                thoughts: thoughts.length > 0 ? thoughts : undefined,
                thinkingSeconds: seconds,
              }
            : m,
        ),
      }));
    }
    if (session) {
      try {
        await cancelAgentTurn(effectiveBase, settingsRef.current.apiKey, session.id);
      } catch {
        /* turn is already stopped locally; server cancel is best-effort */
      }
    }
  }, [effectiveBase, updateConversation]);

  // K2 — Esc stops the running turn (WinForms ProcessCmdKey Esc).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && streamingRef.current && !settingsOpenRef.current && !loginOpenRef.current) {
        e.preventDefault();
        void stopTurn();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [stopTurn]);

  const sendMessage = useCallback(
    async (text: string, attachments: Attachment[]): Promise<boolean> => {
      // --- No-account gate: chat with no Muse account connected opens the
      // login dialog (normal mode, clear_session: false) instead of
      // sending anything to the gateway. Returns false so the caller
      // keeps the draft; true once a turn was actually started.
      //
      // Detection source (chosen): a fresh GET /admin/accounts AT SEND
      // TIME, judged by the existing hasSavedLogin() rule via
      // anySavedLogin() — the same list App keeps for the welcome pill
      // (loaded when the backend becomes healthy, refreshed when
      // Settings/LoginDialog close). The gate re-fetches instead of
      // trusting that list so a login that completed seconds ago, or an
      // account deleted elsewhere, is never judged from a stale
      // snapshot. /readyz was NOT used: it only says the gateway is up,
      // nothing about accounts.
      // Fail-open: any fetch failure (backend down, key missing) means
      // "unknown", so the chat proceeds exactly as before — unknown is
      // never mistaken for "no account". Quota / out-of-credit is NOT
      // checked here: a connected but exhausted account still goes
      // through and gets the existing waterfall 503.
      if (loginOpenRef.current) {
        // LoginDialog is already open — never open a second one. The
        // draft stays because we return false without sending.
        return false;
      }
      try {
        const res = await fetchAccounts(effectiveBase, settingsRef.current.apiKey);
        const accounts = (unwrapData<GatewayAccount[]>(res) ?? []) as GatewayAccount[];
        // Keep App's list (and the welcome pill) in sync with the fresh
        // check — a login that just completed shows up immediately.
        setAccounts(accounts);
        setAccountsLoaded(true);
        if (!anySavedLogin(accounts)) {
          // Draft preservation, two layers: (1) returning false makes
          // ChatPanel keep its draft + attachments; (2) composerPrefill
          // puts the text back into the composer for callers with no
          // composer of their own (WorkspacePanel "Send to agent").
          setComposerPrefill({ text, at: Date.now() });
          openLogin({ clearSession: false, notice: t(settingsRef.current.uiLanguage, "loginNeededToChat") });
          return false;
        }
      } catch {
        /* account list unknown — fail open, chat proceeds as before */
      }
      const conv = active ?? createConversation();
      const replyLanguage = detectReplyLanguage(
        text,
        settingsRef.current.followQuestionLanguage,
        settingsRef.current.uiLanguage,
      );
      const userMsg: ChatMessage = {
        id: newMessageId(),
        role: "user",
        content: text,
        createdAt: Date.now(),
        replyLanguage,
        attachments: attachments.length > 0 ? attachments : undefined,
      };
      const assistantMsg: ChatMessage = {
        id: newMessageId(),
        role: "assistant",
        content: "",
        createdAt: Date.now(),
        replyLanguage,
      };
      const title =
        conv.messages.length === 0
          ? text.slice(0, 48) + (text.length > 48 ? "…" : "")
          : conv.title;
      updateConversation(conv.id, (c) => ({
        ...c,
        title,
        model: settingsRef.current.model,
        updatedAt: Date.now(),
        messages: [...c.messages, userMsg, assistantMsg],
      }));
      setActiveId(conv.id);
      setStreaming(true);
      setStreamingText("");
      setStreamingPlan(null);
      setStreamingActions(null);
      setStreamingThoughts([]);
      setPendingApproval(null);
      setGoalActive(/^\s*\/goal(\s|$)/i.test(text));
      const token = ++turnTokenRef.current;
      turnStartedAtRef.current = Date.now();
      turnAssistantIdRef.current = assistantMsg.id;
      turnConvIdRef.current = conv.id;

      // Prompt for this turn: user text + inlined text attachments. The
      // agent server keeps its own history, so only this turn is sent.
      // C13 — pending editor-save notes ride along as silent context.
      let prompt = text;
      if (editorContextRef.current.length > 0) {
        prompt = `${editorContextRef.current.join("\n")}\n\n${prompt}`;
        editorContextRef.current = [];
      }
      for (const a of attachments) {
        if (a.kind === "text" && a.text !== null) {
          prompt += `\n\n[Attached file: ${a.name}]\n${a.text}`;
        } else if (a.kind === "image") {
          prompt += `\n\n[Attached image: ${a.name}]`;
        }
      }
      const images = attachments
        .filter((a) => a.kind === "image" && a.dataUrl)
        .slice(0, 4)
        .map((a) => a.dataUrl as string);

      const finish = (
        finalText: string,
        plan: ToolPlanAction[] | null,
        error?: string,
        usage?: { in: number; out: number },
        actions?: ToolActionState[],
        turnStatus?: string,
      ) => {
        if (token !== turnTokenRef.current) return; // superseded (Stop)
        const seconds = (Date.now() - turnStartedAtRef.current) / 1000;
        const thoughts = [...streamingThoughtsRef.current];
        setStreaming(false);
        setStreamingText("");
        setStreamingPlan(null);
        setStreamingActions(null);
        setPendingApproval(null);
        setGoalActive(false);
        if (error) {
          // A16 — keep the draft so a failed send does not eat the text.
          setFailedDraft({ text, at: Date.now() });
          console.error("[muse] turn failed:", error);
        }
        updateConversation(conv.id, (c) => ({
          ...c,
          updatedAt: Date.now(),
          messages: c.messages.map((m) =>
            m.id === assistantMsg.id
              ? {
                  ...m,
                  // Display-only: strip a raw `json {"status":...,"summary":...}`
                  // envelope so the saved message shows just the summary.
                  // Agent-loop parsing of actions/status is untouched.
                  content: unwrapReplyEnvelope(finalText),
                  toolPlan: plan ?? undefined,
                  agentActions: actions ?? undefined,
                  error,
                  turnStatus,
                  thoughts: thoughts.length > 0 ? thoughts : undefined,
                  thinkingSeconds: thoughts.length > 0 ? seconds : undefined,
                  tokensIn: usage?.in,
                  tokensOut: usage?.out,
                }
              : m,
          ),
        }));
      };

      // --- Legacy plain-chat path (no agent endpoints on this gateway) ---
      const runLegacyChat = async (fallbackNote?: string) => {
        const history = [...conv.messages, userMsg]
          .filter((m) => m.role !== "system")
          .map((m) => {
            let content = m.content;
            if (m.attachments) {
              for (const a of m.attachments) {
                if (a.kind === "text" && a.text !== null) {
                  content += `\n\n[Attached file: ${a.name}]\n${a.text}`;
                } else {
                  content += `\n\n[Attached file: ${a.name}]`;
                }
              }
            }
            return { role: m.role as "user" | "assistant", content };
          });
        await streamChatCompletion(
          effectiveBase,
          settingsRef.current.apiKey,
          {
            model: settingsRef.current.model,
            messages: history,
            conversationId: conv.id,
            replyLanguage,
          },
          {
            onDelta: (full) => setStreamingText(streamingReplyDisplay(full)),
            onDone: (full, usage) => {
              const plan = detectToolPlan(full);
              const note = fallbackNote ? `${fallbackNote}\n\n` : "";
              if (plan) {
                finish(
                  note +
                    (settingsRef.current.uiLanguage === "id"
                      ? "Rencana tool diterima — lihat bagian Tool plan di bawah."
                      : "Tool plan received — see the Tool plan section below."),
                  plan,
                  undefined,
                  usage,
                );
              } else {
                // Unwrap before prepending the fallback note, so a raw
                // envelope still strips even when a note is present.
                finish(note + unwrapReplyEnvelope(full), null, undefined, usage);
              }
            },
            onError: (message) => {
              // A14 — friendly error text, not the raw gateway message.
              finish("", null, friendlyError(message, settingsRef.current.uiLanguage));
            },
          },
        );
      };

      // --- Agent path (primary when a workspace is set) ---
      // Resolve the folder first: a stale "/home/user/workspace" (or an
      // empty setting) becomes a real Windows folder, created if missing,
      // so the first turn can never 400 with "workspace not found".
      let workspace = settingsRef.current.workspacePath;
      try {
        const resolved = await invoke<string>("resolve_workspace", {
          requested: workspace || null,
        });
        // Defect B: normalise + compare canonically — never write a
        // `\\?\`-spelled path back into settings (that is what orphaned
        // the FOX conversation list: same folder, different key).
        const normalised = normalizeWorkspacePath(resolved);
        if (normalised && !sameWorkspacePath(normalised, workspace)) {
          workspace = normalised;
          setSettings((prev) => ({ ...prev, workspacePath: normalised }));
        } else if (normalised) {
          workspace = normalised;
        }
      } catch {
        /* keep the configured path; the server creates it if missing */
      }
      if (!workspace) {
        await runLegacyChat();
        return true;
      }
      const isAgentMissing = (e: unknown) =>
        e instanceof Error && (e as Error & { status?: number }).status === 404;

      const syncChanges = async (sessionId: string) => {
        try {
          const { changes: serverChanges } = await getAgentChanges(effectiveBase, settingsRef.current.apiKey, sessionId);
          if (serverChanges.length > 0) {
            setChanges((prev) => {
              const next = [...prev];
              for (const sc of serverChanges) {
                const id = `agent-${sessionId}-${sc.path}`;
                if (!next.some((c) => c.id === id)) {
                  next.unshift({
                    id,
                    rel: sc.path,
                    kind: "write",
                    createdAt: Date.now(),
                    previousContent: null,
                    by: "agent",
                    agentSessionId: sessionId,
                    diff: sc.diff || undefined,
                  });
                }
              }
              return next.slice(0, 100);
            });
          }
          setFilesVersion((v) => v + 1);
        } catch {
          /* changes sync is best-effort */
        }
      };

      const pushThought = (line: string) => {
        const trimmed = line.trim();
        if (!trimmed) return;
        setStreamingThoughts((prev) => {
          if (prev.length > 0 && prev[prev.length - 1] === trimmed) return prev;
          return [...prev, trimmed].slice(-50);
        });
      };

      try {
        let cached = agentSessionsRef.current[conv.id];
        if (!cached || !sameWorkspacePath(cached.workspace, workspace)) {
          const session = await createAgentSession(effectiveBase, settingsRef.current.apiKey, {
            workspace,
            shell: settingsRef.current.shell || undefined,
            model: settingsRef.current.model,
            replyLanguage,
            turbo: settingsRef.current.turboMode === "turbo",
          });
          cached = { id: session.session_id, workspace };
          agentSessionsRef.current[conv.id] = cached;
          saveAgentSessions();
        }
        const sessionId = cached.id;

        // Stream the turn; resolves on the `done` event.
        let full = "";
        const actions: ToolActionState[] = [];
        const outcome = await new Promise<{ ok: boolean; summary: string; status: string }>((resolve, reject) => {
          const pushActions = () => setStreamingActions([...actions]);
          const markTool = (tool: string, status: ToolActionState["status"], result?: string) => {
            for (let i = actions.length - 1; i >= 0; i--) {
              if (actions[i].tool === tool && (actions[i].status === "running" || actions[i].status === "pending")) {
                actions[i] = { ...actions[i], status, ...(result !== undefined ? { result } : {}) };
                pushActions();
                return;
              }
            }
          };
          void streamAgentTurn(
            effectiveBase,
            settingsRef.current.apiKey,
            sessionId,
            { prompt, images, replyLanguage, turbo: settingsRef.current.turboMode === "turbo" },
            {
              onEvent: (ev: AgentTurnEvent) => {
                switch (ev.event) {
                  case "assistant_start":
                    if (ev.text) pushThought(ev.text);
                    break;
                  case "assistant_delta":
                    if (ev.text) {
                      full += ev.text;
                      // Hide a raw JSON envelope while it streams in;
                      // show just its summary once it parses.
                      setStreamingText(streamingReplyDisplay(full));
                    }
                    break;
                  case "assistant":
                    if (ev.text) {
                      full = ev.text;
                      setStreamingText(streamingReplyDisplay(full));
                    }
                    break;
                  case "tool_start":
                    actions.push({
                      tool: ev.tool ?? "tool",
                      description: ev.text,
                      path: ev.path,
                      status: "running",
                    });
                    pushActions();
                    pushThought(activityLine(ev.tool ?? "tool", ev.path, ev.text ?? "", settingsRef.current.uiLanguage));
                    break;
                  case "command_output":
                    for (let i = actions.length - 1; i >= 0; i--) {
                      if (actions[i].tool === "run_command" && actions[i].status === "running") {
                        actions[i] = { ...actions[i], result: (actions[i].result ?? "") + (ev.text ?? "") };
                        pushActions();
                        break;
                      }
                    }
                    break;
                  case "tool_result":
                    markTool(
                      ev.tool ?? "tool",
                      (ev.text ?? "").startsWith("error:") ? "error" : "done",
                      ev.text,
                    );
                    break;
                  case "skill":
                    actions.push({ tool: "skill", description: ev.text, status: "done" });
                    pushActions();
                    pushThought(ev.text ?? "skill");
                    break;
                  case "goal":
                    setGoalActive(true);
                    if (ev.text) pushThought(ev.text);
                    break;
                  case "protocol_error":
                  case "protocol_retry":
                  case "context_compacted":
                    full += `\n[${ev.event}: ${ev.text ?? ""}]`;
                    setStreamingText(full);
                    break;
                  case "approval_required":
                    if (ev.approval_id) {
                      const approval: PendingApproval = {
                        approvalId: ev.approval_id,
                        tool: ev.tool ?? "tool",
                        description: ev.description ?? ev.text ?? "",
                        preview: ev.preview ?? "",
                        path: ev.path,
                      };
                      setPendingApproval(approval);
                      for (let i = actions.length - 1; i >= 0; i--) {
                        if (actions[i].tool === approval.tool && actions[i].status === "running") {
                          actions[i] = {
                            ...actions[i],
                            status: "awaiting_approval",
                            approvalId: approval.approvalId,
                            description: approval.description,
                            preview: approval.preview,
                          };
                          pushActions();
                          break;
                        }
                      }
                    }
                    break;
                  case "done":
                    resolve({ ok: ev.ok ?? false, summary: ev.summary ?? "", status: ev.status ?? "" });
                    break;
                  case "error":
                    reject(new Error(ev.text ?? "Agent turn failed."));
                    break;
                  default:
                    break; // completed/incomplete/cancelled/declined/waiting_input/goal: covered by `done`
                }
              },
              onError: (message, status) =>
                reject(Object.assign(new Error(message), status !== undefined ? { status } : {})),
            },
          );
        });
        await syncChanges(sessionId);
          const finalText = full.trim().length > 0 ? full : outcome.summary;
          if (!outcome.ok) {
            if (outcome.status === "waiting_input") {
              finish(finalText, null, undefined, undefined, [...actions]);
            } else {
              // A6 — cancelled / declined / incomplete get a notice card,
              // not a red error box.
              finish(finalText, null, undefined, undefined, [...actions], outcome.status || "incomplete");
            }
          } else {
            finish(finalText, null, undefined, undefined, [...actions]);
          }
      } catch (e) {
        if (token !== turnTokenRef.current) return true; // stopped meanwhile — turn was accepted, draft already sent
        if (isAgentMissing(e)) {
          // Old gateway without /v1/agent/* — fall back to plain chat once.
          // Drop the cached session too: a 404 can also mean the server
          // restarted and no longer knows this session id.
          delete agentSessionsRef.current[conv.id];
          saveAgentSessions();
          await runLegacyChat(t(settingsRef.current.uiLanguage, "agentUnavailableFallback"));
        } else {
          const raw = e instanceof Error ? e.message : String(e);
          finish("", null, `${friendlyError(raw, settingsRef.current.uiLanguage)}`);
        }
      }
      return true;
    },
    [active, createConversation, effectiveBase, updateConversation],
  );

  const handleApprove = useCallback(
    async (approvalId: string, approved: boolean) => {
      const approval = pendingApprovalRef.current;
      const convId = activeId;
      if (!approval || approval.approvalId !== approvalId || !convId) return;
      const session = agentSessionsRef.current[convId];
      if (!session) return;
      setStreamingActions((prev) =>
        (prev ?? []).map((a) =>
          a.approvalId === approvalId
            ? { ...a, status: approved ? "running" : "denied" }
            : a,
        ),
      );
      setPendingApproval(null);
      try {
        await approveAgentTool(effectiveBase, settingsRef.current.apiKey, session.id, approvalId, approved);
      } catch (e) {
        // The turn may already have moved on; surface it on the message.
        setStreamingText((prev) => `${prev}\n[approval error: ${e instanceof Error ? e.message : String(e)}]`);
      }
    },
    [activeId, effectiveBase],
  );

  // J1 — deny the pending action AND stop the whole turn.
  const handleAbortApproval = useCallback(
    async (approvalId: string) => {
      await handleApprove(approvalId, false);
      await stopTurn();
    },
    [handleApprove, stopTurn],
  );

  const revertAgentChange = useCallback(
    async (c: FileChange) => {
      if (!c.agentSessionId) return;
      await apiRevertAgentChange(effectiveBase, settingsRef.current.apiKey, c.agentSessionId, c.rel);
      setChanges((prev) => prev.filter((x) => x.id !== c.id));
      setFilesVersion((v) => v + 1);
    },
    [effectiveBase],
  );

  // C1 — folder picker for the welcome-screen CTA (same helper as the
  // sidebar Browse button; falls back to manual input when the dialog
  // plugin is not registered on the Rust side yet).
  const chooseProject = useCallback(async () => {
    try {
      const picked = await pickFolder(settingsRef.current.workspacePath || undefined);
      if (picked) setSettings((s) => ({ ...s, workspacePath: normalizeWorkspacePath(picked) }));
    } catch (e) {
      setBackendError(
        e instanceof DialogPluginMissingError
          ? t(settingsRef.current.uiLanguage, "folderPickerMissing")
          : String(e),
      );
    }
  }, []);

  // Sync Turbo / reply-language preferences to the gateway when settings
  // are saved (fire-and-forget; old gateways without the endpoint ignore it).
  // A changed driver restarts the backend so the new driver actually
  // applies (the driver is fixed at sidecar spawn time).
  const saveSettingsAndSync = useCallback((s: Settings) => {
    const prevDriver = settingsRef.current.driver;
    const wasRunning = backendRef.current.running;
    // Defect B: never persist a verbatim / oddly-spelled workspace path.
    const normalised = { ...s, workspacePath: normalizeWorkspacePath(s.workspacePath) };
    setSettings(normalised);
    const replyLanguage = s.uiLanguage === "id" ? "Bahasa Indonesia" : "English";
    void updateAgentPreferences(s.backendBaseUrl, s.apiKey || settingsRef.current.apiKey, {
      turbo: s.turboMode === "turbo",
      read_only: s.turboMode === "readonly",
      follow_question_language: s.followQuestionLanguage,
      reply_language: replyLanguage,
      ui_language: replyLanguage,
    }).catch(() => {
      /* old gateway or unreachable — local settings still apply */
    });
    if (s.driver !== prevDriver && wasRunning) {
      void (async () => {
        try {
          await invoke("stop_backend");
          const st = await invoke<BackendStatus>("start_backend", {
            backendPath: null,
            driver: s.driver,
          });
          setBackend(st);
          setBackendError(null);
        } catch (e) {
          setBackendError(String(e));
        }
      })();
    }
  }, []);

  const exportTranscript = useCallback(() => {
    if (!active) return;
    const lines = [`# ${active.title}`, ""];
    for (const m of active.messages) {
      lines.push(`## ${m.role === "user" ? t(lang, "you") : t(lang, "museAgent")} — ${new Date(m.createdAt).toLocaleString()}`);
      lines.push("");
      lines.push(m.error ?? m.content);
      lines.push("");
    }
    download(`${active.title.replace(/[^\w\-]+/g, "-").slice(0, 40) || "conversation"}.md`, lines.join("\n"), "text/markdown");
  }, [active, lang]);

  const exportJson = useCallback(() => {
    if (!active) return;
    download(`${active.id}.json`, JSON.stringify(active, null, 2), "application/json");
  }, [active]);

  const importFile = useCallback(async (file: File) => {
    try {
      const parsed = JSON.parse(await file.text()) as Conversation;
      if (!parsed || typeof parsed !== "object" || !Array.isArray(parsed.messages)) {
        throw new Error("Not a conversation JSON (missing messages array).");
      }
      const conv: Conversation = {
        id: parsed.id && typeof parsed.id === "string" ? parsed.id : newConversation().id,
        title: typeof parsed.title === "string" ? parsed.title : "Imported conversation",
        createdAt: typeof parsed.createdAt === "number" ? parsed.createdAt : Date.now(),
        updatedAt: Date.now(),
        pinned: !!parsed.pinned,
        messages: parsed.messages.filter((m) => m && typeof m.content === "string"),
      };
      setConversations((prev) => [conv, ...prev.filter((c) => c.id !== conv.id)]);
      setActiveId(conv.id);
    } catch (e) {
      // No blocking alert: surface inline under the backend status instead.
      setBackendError(String(e));
    }
  }, []);

  const recordChange = useCallback((c: FileChange) => {
    setChanges((prev) => [c, ...prev].slice(0, 100));
  }, []);

  const undoChange = useCallback((c: FileChange) => {
    if (c.by === "agent") {
      // Server-side revert (WorkspacePanel already branches here, but a
      // direct call is also safe): revert then drop from the list.
      void revertAgentChange(c).catch(() => {
        /* revert failed — keep the entry so the user can retry */
      });
      return;
    }
    setChanges((prev) => prev.filter((x) => x.id !== c.id));
  }, [revertAgentChange]);

  // I11 — diagnostics text for Settings → Advanced (no secrets: keys
  // are never included).
  const diagnostics = [
    `Backend: ${backend.running ? "running" : "stopped"} · healthy=${backend.healthy} · ${backend.base_url ?? settings.backendBaseUrl}`,
    `Gateway: ${gatewayOk ? "reachable" : "unreachable"} · model=${settings.model} · driver=${settings.driver} · turbo=${settings.turboMode}`,
    `Workspace: ${settings.workspacePath || "(none)"} · conversations=${conversations.length}`,
    backendError ? `Last error: ${backendError}` : "No setup errors.",
  ].join("\n");

  return (
    <main className="flex h-full w-full flex-col overflow-hidden bg-[#17171D] text-[#F5F5F7]">
      {/* I13 — update banner (dismissed per version, trusted links only). */}
      {availableUpdate && (
        <div className="flex items-center gap-2 border-b border-[#3867F5]/40 bg-[#3867F5]/15 px-4 py-2 text-[12px] text-[#c7d6ff]">
          <span className="flex-1">
            {t(lang, "updateAvailable").replace("{0}", `v${availableUpdate.version}`)}
          </span>
          <Button
            size="sm"
            className="h-7 bg-[#3867F5] text-white hover:bg-[#2f5ae0]"
            onClick={() => void openUrl(availableUpdate.url)}
          >
            {t(lang, "viewUpdate")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 text-[#c7d6ff] hover:text-white"
            onClick={() => {
              dismissUpdate(availableUpdate.version);
              setAvailableUpdate(null);
            }}
          >
            {t(lang, "dismissUpdate")}
          </Button>
          <button
            onClick={() => setAvailableUpdate(null)}
            className="text-[#c7d6ff] hover:text-white"
            aria-label={t(lang, "close")}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}
      <ResizablePanelGroup direction="horizontal" className="h-full w-full min-h-0 flex-1">
        <ResizablePanel defaultSize={19} minSize={15} maxSize={26} className="min-h-0">
          <Sidebar
            lang={lang}
            conversations={conversations}
            activeId={activeId}
            onSelect={selectConversation}
            onNew={() => void createConversation()}
            onPin={(id) => updateConversation(id, (c) => ({ ...c, pinned: !c.pinned }))}
            onDelete={(id) => {
              const session = agentSessionsRef.current[id];
              if (session) {
                delete agentSessionsRef.current[id];
                saveAgentSessions();
                void deleteAgentSession(effectiveBase, settingsRef.current.apiKey, session.id).catch(() => {
                  /* best-effort cleanup */
                });
              }
              // Guard (4 Nov 2026): deleting the LAST conversation is the
              // one explicit user action allowed to persist an empty list
              // — the store-level guard blocks every other [] write, so
              // the intent must be declared here, before the save effect
              // runs. Deleting one of several needs no flag (list stays
              // non-empty).
              const remaining = convsRef.current.filter((c) => c.id !== id);
              setConversations(remaining);
              if (remaining.length === 0) {
                saveConversations([], convWsRef.current, { allowEmptyOverwrite: true });
              }
              if (activeId === id) setActiveId(null);
            }}
            models={models}
            model={settings.model}
            onModelChange={(m) => {
              setSettings((s) => ({ ...s, model: m }));
              if (activeId) updateConversation(activeId, (c) => ({ ...c, model: m }));
            }}
            workspacePath={settings.workspacePath}
            onWorkspaceChange={(p) => setSettings((s) => ({ ...s, workspacePath: normalizeWorkspacePath(p) }))}
            backend={backend}
            backendError={backendHealthy ? null : backendError}
            onStartBackend={() => void startBackend()}
            onStopBackend={() => void stopBackend()}
            onRefreshBackend={() => void refreshBackend()}
            backendBaseUrl={effectiveBase}
            apiKey={settings.apiKey}
            accountConnected={accountConnected}
            onOpenSettings={() => setSettingsOpen(true)}
            onOpenLogin={() => openLogin()}
            onExportTranscript={exportTranscript}
            onExportJson={exportJson}
            onImportFile={(f) => void importFile(f)}
          />
        </ResizablePanel>
        <ResizableHandle withHandle className="bg-[#343541]" />
        <ResizablePanel defaultSize={59} minSize={30} className="min-h-0">
          <ChatPanel
            lang={lang}
            conversation={active}
            streaming={streaming}
            streamingText={streamingText}
            displayedTextRef={streamingDisplayedRef}
            streamingPlan={streamingPlan}
            streamingActions={streamingActions}
            streamingThoughts={streamingThoughts}
            goalActive={goalActive}
            pendingApproval={pendingApproval}
            onApprove={(approvalId, approved) => void handleApprove(approvalId, approved)}
            onAbortApproval={(approvalId) => void handleAbortApproval(approvalId)}
            onStop={() => void stopTurn()}
            onViewTerminal={() => {
              setWorkspaceTab("terminal");
              setRightCollapsed(false);
            }}
            onChooseProject={() => void chooseProject()}
            backendHealthy={backendHealthy}
            accountConnected={accountConnected}
            backendStarting={backendStarting}
            hasWorkspace={!!settings.workspacePath}
            onStartBackend={() => void startBackend()}
            onOpenSettings={() => setSettingsOpen(true)}
            onOpenLogin={() => openLogin()}
            onSend={(text, atts) => sendMessage(text, atts)}
            failedDraft={failedDraft}
            composerPrefill={composerPrefill}
            model={settings.model}
          />
        </ResizablePanel>
        <ResizableHandle withHandle className="bg-[#343541]" />
        {rightCollapsed ? (
          <div className="flex w-9 shrink-0 flex-col items-center bg-[#1D1D26] pt-2">
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 text-[#A1A1AA]"
              title={t(lang, "expand")}
              onClick={() => setRightCollapsed(false)}
            >
              <PanelRightOpen className="h-4 w-4" />
            </Button>
          </div>
        ) : (
          <ResizablePanel defaultSize={22} minSize={18} maxSize={32} className="min-h-0">
            <WorkspacePanel
              lang={lang}
              settings={settings}
              backendBaseUrl={effectiveBase}
              changes={changes}
              onRecordChange={recordChange}
              onUndoChange={undoChange}
              onRevertAgentChange={(c) => revertAgentChange(c)}
              refreshSignal={filesVersion}
              onShellChange={(shell) => setSettings((s) => ({ ...s, shell }))}
              onCollapsedChange={setRightCollapsed}
              collapsed={rightCollapsed}
              tab={workspaceTab}
              onTabChange={setWorkspaceTab}
              onAskMuse={(prompt) => setComposerPrefill({ text: prompt, at: Date.now() })}
              onSendToAgent={(text) => void sendMessage(text, [])}
              onDirtyChange={reportEditorDirty}
              onNoteEditorChange={(rel) => {
                editorContextRef.current = [
                  ...editorContextRef.current,
                  `[Editor: the user saved changes to "${rel}" in the editor.]`,
                ].slice(-5);
              }}
            />
          </ResizablePanel>
        )}
      </ResizablePanelGroup>

      <SettingsDialog
        lang={lang}
        open={settingsOpen}
        // Accounts may have been added / deleted / toggled inside
        // Settings — refresh App's list so the welcome pill follows.
        onClose={() => {
          setSettingsOpen(false);
          void refreshAccounts();
        }}
        settings={settings}
        backendBaseUrl={effectiveBase}
        onSave={saveSettingsAndSync}
        backendError={backendHealthy ? null : backendError}
        backendHealthy={backendHealthy}
        diagnostics={diagnostics}
        // WinForms PreferencesForm closes before the login form opens.
        onOpenLogin={(opts) => {
          setSettingsOpen(false);
          openLogin({ clearSession: opts.clearSession, label: opts.initialLabel });
        }}
      />
      <LoginDialog
        key={loginSeq}
        lang={lang}
        open={loginOpen}
        // A sign-in may have just completed inside the dialog — refresh
        // App's list so the welcome pill flips to Connected without
        // waiting for the next send attempt.
        onClose={() => {
          setLoginOpen(false);
          void refreshAccounts();
        }}
        settings={settings}
        backend={backend}
        initialLabel={loginRequest?.label}
        clearSession={loginRequest?.clearSession ?? false}
        notice={loginRequest?.notice ?? null}
      />
    </main>
  );
}

export default App;
