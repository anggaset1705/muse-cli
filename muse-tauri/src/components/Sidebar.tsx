// Left sidebar: brand, working toolbar, New Conversation, search,
// grouped history (PINNED / TODAY / YESTERDAY / THIS WEEK / OLDER),
// and the bottom block: model selector, workspace, Settings, Login,
// backend status, Import / Transcript / JSON.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  FileJson,
  Folder,
  FolderOpen,
  MessageSquare,
  Pin,
  PinOff,
  Plus,
  Search,
  Settings,
  Trash2,
  Upload,
  FileText,
  LogIn,
  Play,
  Square,
  RefreshCw,
  Server,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { BackendStatus, Conversation, GatewayAccount } from "@/lib/types";
import { t, tf, type Lang } from "@/lib/i18n";
import { fetchAccounts, fetchQuota, unwrapData } from "@/lib/gateway";
import { DialogPluginMissingError, pickFolder } from "@/lib/dialog";
import logoUrl from "@/assets/muse-logo.png";

interface Props {
  lang: Lang;
  conversations: Conversation[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onPin: (id: string) => void;
  onDelete: (id: string) => void;
  models: string[];
  model: string;
  onModelChange: (m: string) => void;
  workspacePath: string;
  onWorkspaceChange: (p: string) => void;
  backend: BackendStatus;
  /** Inline backend error (local start/stop failure, or Rust's backend.error). */
  backendError: string | null;
  onStartBackend: () => void;
  onStopBackend: () => void;
  onRefreshBackend: () => void;
  onOpenSettings: () => void;
  onOpenLogin: () => void;
  onExportTranscript: () => void;
  onExportJson: () => void;
  onImportFile: (file: File) => void;
  /** Live gateway URL (random port) + synced API key, for the Usage widget. */
  backendBaseUrl: string;
  apiKey: string;
  /** Same source as the welcome pill (App: accountsLoaded && anySavedLogin).
   * When false the Usage widget must not fetch or show numbers. */
  accountConnected: boolean;
}

/** Last-known usage, kept in localStorage so the widget is not blank
 *  while fresh data loads (and so a failed fetch still shows history). */
interface UsageData {
  plan: string | null;
  weeklyPct: number | null;
  resetsAt: string | null;
  extraLeft: string | null;
  fetchedAt: number;
}

const USAGE_CACHE_KEY = "muse-usage-cache-v1";

function loadUsageCache(): UsageData | null {
  try {
    const raw = localStorage.getItem(USAGE_CACHE_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as UsageData;
    if (typeof v.fetchedAt !== "number") return null;
    return v;
  } catch {
    return null;
  }
}

/** Clears the last-known usage (state + localStorage) when the account
 *  list is empty — a stale cached number must never be shown as if it
 *  were live. Transient fetch failures do NOT clear it (a failed fetch
 *  still shows history); only "no account" does. */
function clearUsageCache() {
  try {
    localStorage.removeItem(USAGE_CACHE_KEY);
  } catch {
    /* storage unavailable — nothing to clear */
  }
}

function groupOf(c: Conversation, now: Date): string {
  if (c.pinned) return "pinned";
  const d = new Date(c.updatedAt);
  const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const dayMs = 86400000;
  const diff = startOfDay(now) - startOfDay(d);
  if (diff === 0) return "today";
  if (diff === dayMs) return "yesterday";
  if (diff < dayMs * 7) return "thisWeek";
  return "older";
}

const GROUP_ORDER = ["pinned", "today", "yesterday", "thisWeek", "older"] as const;

export function Sidebar(props: Props) {
  const { lang } = props;
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<"all" | "pinned">("all");
  const [pickerError, setPickerError] = useState<string | null>(null);
  const importRef = useRef<HTMLInputElement>(null);
  const now = new Date();

  // --- Usage widget (below MODEL) ---------------------------------------
  // Reads the Muse web usage panel through the gateway
  // (GET /admin/accounts/{id}/quota — the browser driver opens the usage
  // dialog in the agent tab, up to ~12 s, and may mark the tab stale).
  // Fetch ONCE when the backend becomes healthy (app launch: no chat turn
  // is in flight yet) + on manual Refresh; the server caches 30 min, so
  // re-mounts do not re-drive the tab. Last-known value is shown while
  // fresh data loads.
  const [usage, setUsage] = useState<UsageData | null>(() => loadUsageCache());
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageError, setUsageError] = useState<string | null>(null);
  const usageLoadedFor = useRef("");
  const loadUsage = useCallback(async () => {
    const base = props.backendBaseUrl;
    const key = props.apiKey;
    if (!base || !key) {
      setUsageError(t(lang, "usageUnavailable"));
      return;
    }
    setUsageLoading(true);
    setUsageError(null);
    try {
      const accounts = (unwrapData<GatewayAccount[]>(await fetchAccounts(base, key)) ??
        []) as GatewayAccount[];
      const acc = accounts.find((a) => a.enabled !== false) ?? accounts[0];
      if (!acc) {
        // No account at all — NOT a transient failure: drop the stale
        // cached numbers so the widget (and the next app restart) shows
        // "not connected" instead of ghost figures.
        setUsage(null);
        clearUsageCache();
        setUsageError(t(lang, "welcomeNotConnected"));
        return;
      }
      const body = (await fetchQuota(base, key, acc.id)) as {
        quota?: Record<string, unknown>;
      };
      const q = body?.quota ?? {};
      if (typeof q.error === "string" && q.error) throw new Error(q.error);
      const pct = typeof q.weekly_used_pct === "number" ? q.weekly_used_pct : null;
      if (!q.found && pct === null && typeof q.plan !== "string") {
        // Parser returned nothing usable (e.g. dialog in an unknown language).
        setUsage(null);
        setUsageError(t(lang, "usageUnavailable"));
        return;
      }
      const data: UsageData = {
        plan: typeof q.plan === "string" ? q.plan : null,
        weeklyPct: pct,
        resetsAt: typeof q.resets_at === "string" ? q.resets_at : null,
        extraLeft: typeof q.extra_left === "string" ? q.extra_left : null,
        fetchedAt: Date.now(),
      };
      setUsage(data);
      try {
        localStorage.setItem(USAGE_CACHE_KEY, JSON.stringify(data));
      } catch {
        /* storage unavailable — widget still works for this session */
      }
    } catch (e) {
      setUsageError(String(e));
    } finally {
      setUsageLoading(false);
    }
  }, [props.backendBaseUrl, props.apiKey, lang]);

  // When the account list goes empty (account deleted / never logged in),
  // drop any cached numbers immediately — the widget must show
  // "not connected", and the next restart must not resurrect them.
  // Resetting usageLoadedFor lets loadUsage run again when an account
  // comes back.
  useEffect(() => {
    if (props.accountConnected) return;
    setUsage(null);
    clearUsageCache();
    usageLoadedFor.current = "";
  }, [props.accountConnected]);

  useEffect(() => {
    if (!props.accountConnected) return;
    if (!props.backend.healthy || !props.backendBaseUrl || !props.apiKey) return;
    const key = `${props.backendBaseUrl}|healthy`;
    if (usageLoadedFor.current === key) return;
    usageLoadedFor.current = key;
    void loadUsage();
  }, [props.accountConnected, props.backend.healthy, props.backendBaseUrl, props.apiKey, loadUsage]);

  // C1 — native folder picker (Tauri dialog plugin). Falls back to the
  // manual path input when the plugin is not registered on the Rust side.
  const browseWorkspace = async () => {
    setPickerError(null);
    try {
      const picked = await pickFolder(props.workspacePath || undefined);
      if (picked) props.onWorkspaceChange(picked);
    } catch (e) {
      setPickerError(
        e instanceof DialogPluginMissingError
          ? t(lang, "folderPickerMissing")
          : String(e),
      );
    }
  };

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return props.conversations.filter((c) => {
      if (filter === "pinned" && !c.pinned) return false;
      if (!q) return true;
      return (
        c.title.toLowerCase().includes(q) ||
        c.messages.some((m) => m.content.toLowerCase().includes(q))
      );
    });
  }, [props.conversations, query, filter]);

  const groups = useMemo(() => {
    const out: Record<string, Conversation[]> = {
      pinned: [],
      today: [],
      yesterday: [],
      thisWeek: [],
      older: [],
    };
    for (const c of visible) out[groupOf(c, now)].push(c);
    return out;
  }, [visible, now]);

  const dot = props.backend.healthy
    ? "bg-emerald-400"
    : props.backend.running
      ? "bg-amber-400"
      : "bg-zinc-500";
  const backendLabel = props.backend.healthy
    ? t(lang, "backendHealthy")
    : props.backend.running
      ? t(lang, "backendStarting")
      : t(lang, "backendStopped");
  // Local error (start/stop/driver-change failure) wins; otherwise show the
  // last failure reason reported by Rust via backend.error.
  // Defect B: a stale failure is NEVER shown while the backend is healthy
  // (a failure from an earlier auto-start during install/swap lingered
  // next to a green backend and read as a current problem).
  const backendErrorText = props.backend.healthy
    ? null
    : (props.backendError ?? props.backend.error ?? null);

  return (
    <div className="flex h-full flex-col bg-[#1D1D26] text-[#F5F5F7]">
      {/* Brand */}
      <div className="flex items-center gap-2.5 px-3 pb-2 pt-3">
        <img src={logoUrl} alt="Muse CLI" className="h-9 w-9 shrink-0 rounded-lg object-cover" />
        <div className="leading-tight">
          <div className="text-[15px] font-semibold">Muse CLI</div>
          <div className="text-[11px] text-[#71717A]">v0.1.0</div>
        </div>
        <span className="ml-auto rounded border border-[#343541] px-1.5 py-0.5 text-[10px] text-[#A1A1AA]">
          {t(lang, "liveBadge")} · {t(lang, "appTagline")}
        </span>
      </div>

      {/* Toolbar: only icons whose function exists */}
      <div className="flex items-center gap-1 px-3 pb-2">
        <Button variant="ghost" size="icon" className="h-8 w-8 text-[#A1A1AA]" title={t(lang, "newConversation")} onClick={props.onNew}>
          <MessageSquare className="h-4 w-4" />
        </Button>
        <Button variant="ghost" size="icon" className="h-8 w-8 text-[#A1A1AA]" title={t(lang, "login")} onClick={props.onOpenLogin}>
          <LogIn className="h-4 w-4" />
        </Button>
        <Button variant="ghost" size="icon" className="h-8 w-8 text-[#A1A1AA]" title={t(lang, "settings")} onClick={props.onOpenSettings}>
          <Settings className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-[#A1A1AA]"
          title={t(lang, "refresh")}
          onClick={props.onRefreshBackend}
        >
          <RefreshCw className="h-4 w-4" />
        </Button>
      </div>

      {/* New conversation */}
      <div className="px-3 pb-2">
        <Button
          className="w-full bg-[#3867F5] text-white hover:bg-[#2f5ae0]"
          onClick={props.onNew}
        >
          <Plus className="h-4 w-4" /> {t(lang, "newConversation")}
          <kbd className="ml-auto rounded bg-white/15 px-1.5 py-0.5 text-[10px]">⌘K</kbd>
        </Button>
      </div>

      {/* Search + filter */}
      <div className="px-3 pb-2">
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-[#71717A]" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
            placeholder={t(lang, "searchConversations")}
            className="border-[#343541] bg-[#292A34] pl-8 text-[13px] text-[#F5F5F7] placeholder:text-[#71717A]"
          />
        </div>
        <div className="mt-2 flex gap-1.5">
          <button
            onClick={() => setFilter("all")}
            className={`rounded-full border px-2.5 py-1 text-[11px] ${
              filter === "all"
                ? "border-[#3867F5] bg-[#3867F5]/20 text-white"
                : "border-[#343541] text-[#A1A1AA]"
            }`}
          >
            {t(lang, "all")}
          </button>
          <button
            onClick={() => setFilter("pinned")}
            className={`rounded-full border px-2.5 py-1 text-[11px] ${
              filter === "pinned"
                ? "border-[#3867F5] bg-[#3867F5]/20 text-white"
                : "border-[#343541] text-[#A1A1AA]"
            }`}
          >
            ★ {t(lang, "pinned")}
          </button>
        </div>
      </div>

      {/* History */}
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {visible.length === 0 && (
          <p className="px-2 py-4 text-center text-[12px] text-[#71717A]">
            {t(lang, "noConversations")}
          </p>
        )}
        {GROUP_ORDER.map((g) => {
          const list = groups[g];
          if (list.length === 0) return null;
          return (
            <div key={g} className="mb-2">
              <div className="flex items-center gap-1 px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-[#71717A]">
                <ChevronDown className="h-3 w-3" />
                {g === "pinned" ? `★ ${t(lang, "pinned")}` : t(lang, g)}
              </div>
              {list.map((c) => (
                <div
                  key={c.id}
                  className={`group flex cursor-pointer items-center gap-1.5 rounded-md px-2 py-1.5 text-[13px] ${
                    c.id === props.activeId
                      ? "bg-[#292A34] text-white"
                      : "text-[#A1A1AA] hover:bg-[#292A34]/60 hover:text-white"
                  }`}
                  onClick={() => props.onSelect(c.id)}
                  title={c.title}
                >
                  <span className="min-w-0 flex-1 truncate">{c.title}</span>
                  <button
                    className="hidden shrink-0 text-[#71717A] hover:text-white group-hover:inline"
                    title={c.pinned ? t(lang, "unpinConversation") : t(lang, "pinConversation")}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onPin(c.id);
                    }}
                  >
                    {c.pinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
                  </button>
                  <button
                    className="hidden shrink-0 text-[#71717A] hover:text-red-400 group-hover:inline"
                    title={t(lang, "deleteConversation")}
                    onClick={(e) => {
                      e.stopPropagation();
                      props.onDelete(c.id);
                    }}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </div>
              ))}
            </div>
          );
        })}
      </div>

      {/* Bottom block */}
      <div className="border-t border-[#343541] px-3 py-2.5">
        <label className="mb-1 block text-[11px] font-semibold uppercase tracking-wide text-[#71717A]">
          {t(lang, "model")}
        </label>
        <select
          value={props.model}
          onChange={(e) => props.onModelChange(e.currentTarget.value)}
          className="w-full rounded-md border border-[#343541] bg-[#292A34] px-2 py-1.5 text-[13px] text-[#F5F5F7]"
        >
          {props.models.map((m) => (
            <option key={m} value={m}>
              {m === "muse-chat" ? "Muse" : m}
            </option>
          ))}
        </select>

        {/* Usage (below MODEL): plan, weekly % + bar, reset date, extra tokens. */}
        <div className="mt-2.5 rounded-md border border-[#343541] bg-[#292A34] px-2.5 py-2">
          <div className="flex items-center justify-between">
            <span className="text-[11px] font-semibold uppercase tracking-wide text-[#71717A]">
              {t(lang, "usage")}
            </span>
            <button
              type="button"
              className="rounded p-1 text-[#A1A1AA] hover:bg-[#343541] hover:text-[#F5F5F7] disabled:opacity-50"
              title={t(lang, "usageRefresh")}
              onClick={() => void loadUsage()}
              disabled={usageLoading || !props.backend.healthy || !props.accountConnected}
            >
              <RefreshCw className={`h-3.5 w-3.5 ${usageLoading ? "animate-spin" : ""}`} />
            </button>
          </div>
          {!props.accountConnected ? (
            <p className="mt-1 text-[12px] text-[#A1A1AA]">{t(lang, "welcomeNotConnected")}</p>
          ) : usageLoading && !usage ? (
            <p className="mt-1 text-[12px] text-[#A1A1AA]">{t(lang, "usageLoading")}</p>
          ) : usage ? (
            <div className="mt-1 space-y-1">
              {usage.plan ? (
                <p className="text-[12px] font-medium text-[#F5F5F7]">{usage.plan}</p>
              ) : null}
              {usage.weeklyPct !== null ? (
                <>
                  <p className="text-[12px] text-[#A1A1AA]">
                    {tf(lang, "usageWeekly", usage.weeklyPct)}
                  </p>
                  <div className="h-1.5 w-full overflow-hidden rounded-full bg-[#343541]">
                    <div
                      className="h-full rounded-full bg-[#3867F5]"
                      style={{ width: `${Math.min(100, Math.max(0, usage.weeklyPct))}%` }}
                    />
                  </div>
                </>
              ) : (
                <p className="text-[12px] text-[#A1A1AA]">—</p>
              )}
              {usage.resetsAt ? (
                <p className="text-[11px] text-[#71717A]">{tf(lang, "usageReset", usage.resetsAt)}</p>
              ) : null}
              {usage.extraLeft ? (
                <p className="text-[11px] text-[#71717A]">{tf(lang, "usageExtra", usage.extraLeft)}</p>
              ) : null}
              <p className="text-[11px] text-[#71717A]">
                {tf(lang, "usageUpdated", new Date(usage.fetchedAt).toLocaleTimeString())}
              </p>
            </div>
          ) : (
            <p className="mt-1 break-words text-[12px] text-[#A1A1AA]" title={usageError ?? undefined}>
              {usageError ?? t(lang, "usageUnavailable")}
            </p>
          )}
        </div>

        <label className="mb-1 mt-2.5 flex items-center gap-1 text-[11px] font-semibold uppercase tracking-wide text-[#71717A]">
          <Folder className="h-3 w-3" /> {t(lang, "workspace")}
        </label>
        <div className="flex gap-1.5">
          <Input
            value={props.workspacePath}
            onChange={(e) => props.onWorkspaceChange(e.currentTarget.value)}
            placeholder="C:\Users\B-STORE\Muse Workspace"
            title={props.workspacePath || t(lang, "workspace")}
            className="min-w-0 flex-1 border-[#343541] bg-[#292A34] font-mono text-[11px] text-[#F5F5F7] placeholder:text-[#71717A]"
          />
          <Button
            variant="secondary"
            size="sm"
            className="shrink-0 bg-[#292A34] text-[#F5F5F7] hover:bg-[#343541]"
            onClick={() => void browseWorkspace()}
            title={t(lang, "chooseFolder")}
          >
            <FolderOpen className="h-3.5 w-3.5" /> {t(lang, "chooseFolder")}
          </Button>
        </div>
        {pickerError && (
          <p className="mt-1 text-[11px] leading-snug text-amber-300" role="status">{pickerError}</p>
        )}

        {/* Backend status — shown only while NOT healthy (starting,
          stopped, or error). When healthy the block is hidden entirely;
          Start/Stop stay inside it for the states that need them. */}
        {!props.backend.healthy && (
          <div className="mt-2.5 flex items-center gap-2 rounded-md border border-[#343541] bg-[#292A34] px-2.5 py-2">
            <Server className="h-4 w-4 shrink-0 text-[#A1A1AA]" />
            <span className={`h-2 w-2 shrink-0 rounded-full ${dot}`} />
            <span className="min-w-0 flex-1 truncate text-[12px] text-[#A1A1AA]" title={backendLabel}>
              {backendLabel}
              {props.backend.base_url ? ` · ${props.backend.base_url}` : ""}
            </span>
            {props.backend.running ? (
              <Button variant="ghost" size="icon" className="h-7 w-7 shrink-0" title={t(lang, "stopBackend")} onClick={props.onStopBackend}>
                <Square className="h-3.5 w-3.5" />
              </Button>
            ) : (
              <Button variant="ghost" size="icon" className="h-7 w-7 shrink-0" title={t(lang, "startBackend")} onClick={props.onStartBackend}>
                <Play className="h-3.5 w-3.5" />
              </Button>
            )}
          </div>
        )}
        {backendErrorText && (
          <p className="mt-1 truncate text-[11px] leading-snug text-red-400" title={backendErrorText}>
            {backendErrorText}
          </p>
        )}

        <div className="mt-2.5 flex gap-1.5">
          <Button variant="secondary" size="sm" className="flex-1 bg-[#292A34] text-[#F5F5F7] hover:bg-[#343541]" onClick={props.onOpenSettings}>
            <Settings className="h-3.5 w-3.5" /> {t(lang, "settings")}
          </Button>
          <Button variant="secondary" size="sm" className="flex-1 bg-[#292A34] text-[#F5F5F7] hover:bg-[#343541]" onClick={props.onExportTranscript} title={t(lang, "exportTranscript")}>
            <FileText className="h-3.5 w-3.5" /> {t(lang, "transcript")}
          </Button>
          <Button variant="secondary" size="sm" className="bg-[#292A34] text-[#F5F5F7] hover:bg-[#343541]" onClick={props.onExportJson} title={t(lang, "exportJson")}>
            <FileJson className="h-3.5 w-3.5" />
          </Button>
          <Button variant="secondary" size="sm" className="bg-[#292A34] text-[#F5F5F7] hover:bg-[#343541]" onClick={() => importRef.current?.click()} title={t(lang, "importConversation")}>
            <Upload className="h-3.5 w-3.5" />
          </Button>
          <input
            ref={importRef}
            type="file"
            accept=".json,application/json"
            className="hidden"
            onChange={(e) => {
              const f = e.currentTarget.files?.[0];
              if (f) props.onImportFile(f);
              e.currentTarget.value = "";
            }}
          />
        </div>
      </div>
    </div>
  );
}
