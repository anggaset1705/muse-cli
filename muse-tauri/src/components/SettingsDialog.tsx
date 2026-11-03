// Settings dialog (shadcn Dialog): language, follow-question-language,
// theme (Dark + Light, ported from DesktopTheme LightPalette), Turbo /
// approval mode, backend base URL + API/admin keys (visible text +
// show/hide + Copy, like WinForms LabeledRow), Accounts & Keys (live
// admin endpoints incl. enable/disable/delete account, create/revoke/
// delete key, quota + renew, admin status), Skills: server skills from
// /v1/agent/skills (import + delete wired) plus the local @alias list,
// Advanced (shell, open workspace folder, diagnostics log), Updates
// (GitHub releases check every 6h, trusted links only).
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { AgentSkill, GatewayAccount, GatewayKey, LocalSkill, MuseDriver, Settings, TurboMode, UiLanguage } from "@/lib/types";
import {
  createApiKey,
  deleteAccount,
  deleteAgentSkill,
  deleteApiKey,
  fetchAccounts,
  fetchAdminStatus,
  fetchAgentSkills,
  fetchKeys,
  fetchQuota,
  anySavedLogin,
  importAgentSkill,
  lastUsedAccountLabel,
  renewAccount,
  setAccountEnabled,
  setAgentSkillEnabled,
  setKeyRevoked,
  unwrapData,
} from "@/lib/gateway";
import { loadSkills, saveSkills } from "@/lib/store";
import { checkForUpdates, dismissUpdate, isTrustedUpdateLink, APP_VERSION, type AvailableUpdate } from "@/lib/updates";
import { t, tf, type Lang } from "@/lib/i18n";

interface Props {
  lang: Lang;
  open: boolean;
  onClose: () => void;
  settings: Settings;
  onSave: (s: Settings) => void;
  /** Live backend base URL from App (`effectiveBase`): the Rust backend
   * runs on a random port, so the stored settings URL (old default 18610)
   * is only a fallback. Same pattern as WorkspacePanel's `backendBaseUrl`. */
  backendBaseUrl: string;
  /** Last backend failure text (I11 diagnostics log). */
  backendError: string | null;
  /** Whether the backend is currently healthy — when true, no failure
   *  text is shown anywhere in this dialog (defect B: a stale failure
   *  from an earlier auto-start must not read as a current problem). */
  backendHealthy: boolean;
  /** Full diagnostics text assembled by App (backend status + errors). */
  diagnostics: string;
  /** Open the sign-in dialog from the Accounts section (WinForms
   * PreferencesForm LoginRequested / AddAccountRequested): Settings closes
   * first, then LoginDialog opens. `clearSession` + `initialLabel` mirror
   * MainForm AddAccount (fresh WebView session, "Account N" label) vs
   * ConnectAccount (restore saved session, last-used label). */
  onOpenLogin?: (opts: { clearSession: boolean; initialLabel: string }) => void;
}

/** Secret/key field: visible text + show/hide + Copy (G1/G2 — WinForms
 * shows the API key as plain text with a Copy button; Bree's explicit
 * complaint was that Tauri hid it and it could not be copied). */
function SecretField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  const [visible, setVisible] = useState(false);
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };
  return (
    <div>
      <label className="mb-1 mt-3 block text-[12px] text-[#A1A1AA]">{label}</label>
      <div className="flex gap-1.5">
        <Input
          type={visible ? "text" : "password"}
          value={value}
          onChange={(e) => onChange(e.currentTarget.value)}
          placeholder={placeholder}
          className="min-w-0 flex-1 border-[#343541] bg-[#292A34] font-mono text-[12px]"
          autoComplete="off"
        />
        <Button
          variant="outline"
          size="sm"
          className="shrink-0 border-[#343541]"
          onClick={() => setVisible(!visible)}
          title={visible ? "Hide" : "Show"}
          aria-label={visible ? "Hide" : "Show"}
        >
          {visible ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="shrink-0 border-[#343541]"
          onClick={() => void copy()}
          disabled={!value}
          title="Copy"
          aria-label="Copy"
        >
          {copied ? <Check className="h-3.5 w-3.5 text-emerald-400" /> : <Copy className="h-3.5 w-3.5" />}
        </Button>
      </div>
      {copied && <p className="mt-1 text-[11px] text-emerald-400">Copied</p>}
    </div>
  );
}

export function SettingsDialog(props: Props) {
  const { lang } = props;
  const [draft, setDraft] = useState<Settings>(props.settings);
  /** Live backend URL (App's `effectiveBase`) — the Rust backend runs on a
   * random port; the stored `draft.backendBaseUrl` (old default 18610) is
   * only a fallback when the backend is not running. */
  const liveBase = props.backendBaseUrl || draft.backendBaseUrl;
  const [accounts, setAccounts] = useState<GatewayAccount[]>([]);
  const [keys, setKeys] = useState<GatewayKey[]>([]);
  /** True once GET /admin/accounts has succeeded at least once. The empty
   * state "(no accounts yet)" is only shown on a successful empty load
   * (WinForms PreferencesForm.RefreshAccountsAsync): on a failed load the
   * old list is kept and the error is shown instead — never claim empty.
   * Also gates the connected-status line: no successful load, no claim. */
  const [accountsLoaded, setAccountsLoaded] = useState(false);
  const [adminError, setAdminError] = useState<string | null>(null);
  const [adminLoading, setAdminLoading] = useState(false);
  const [adminStatus, setAdminStatus] = useState<string | null>(null);
  const [quotaInfo, setQuotaInfo] = useState<Record<string, string>>({});
  const [newKeyPlain, setNewKeyPlain] = useState<string | null>(null);
  const [newKeyCopied, setNewKeyCopied] = useState(false);
  const [skills, setSkills] = useState<LocalSkill[]>(() => loadSkills());
  const [skillAlias, setSkillAlias] = useState("");
  const [skillPath, setSkillPath] = useState("");
  const [serverSkills, setServerSkills] = useState<AgentSkill[]>([]);
  const [serverSkillsError, setServerSkillsError] = useState<string | null>(null);
  const [serverSkillsLoading, setServerSkillsLoading] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const [updateChecking, setUpdateChecking] = useState(false);
  const [availableUpdate, setAvailableUpdate] = useState<AvailableUpdate | null>(null);
  const skillFileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (props.open) {
      setDraft(props.settings);
      setNewKeyPlain(null);
      setAdminStatus(null);
    }
  }, [props.open, props.settings]);

  /** Re-fetch the sidecar's live bearer key before any gateway fetch.
   * The key is regenerated on every backend launch, so this draft — a
   * snapshot of the saved settings — can hold the previous launch's key
   * and the gateway answers 401 ("invalid or missing API key"). The 5s
   * poller in App syncs the saved settings, but not this draft. */
  const syncApiKey = async (): Promise<string> => {
    try {
      const fresh = await invoke<string>("backend_api_key");
      if (fresh) {
        setDraft((d) => (d.apiKey === fresh ? d : { ...d, apiKey: fresh }));
        return fresh;
      }
    } catch {
      /* backend not started by this app — keep the stored key */
    }
    return draft.apiKey;
  };

  const loadServerSkills = async (keyOverride?: string) => {
    const key = keyOverride ?? (await syncApiKey());
    if (!key) {
      setServerSkillsError("API key is empty — set it above first.");
      return;
    }
    setServerSkillsLoading(true);
    setServerSkillsError(null);
    try {
      setServerSkills(await fetchAgentSkills(liveBase, key));
    } catch (e) {
      setServerSkillsError(String(e));
      setServerSkills([]);
    } finally {
      setServerSkillsLoading(false);
    }
  };

  useEffect(() => {
    if (props.open) {
      // Sync the live key first, then load skills + accounts with it —
      // never with the possibly-stale key snapshotted in the draft.
      void (async () => {
        const key = await syncApiKey();
        await loadServerSkills(key);
        await loadAdmin(key);
      })();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.open]);

  const toggleServerSkill = async (alias: string, enabled: boolean) => {
    try {
      await setAgentSkillEnabled(liveBase, draft.apiKey, alias, enabled);
      setServerSkills((prev) => prev.map((s) => (s.alias === alias ? { ...s, enabled } : s)));
    } catch (e) {
      setServerSkillsError(String(e));
    }
  };

  // H3 — import a skill file into the gateway skill store.
  const importSkillFile = async (file: File) => {
    try {
      const content = await file.text();
      await importAgentSkill(liveBase, draft.apiKey, file.name, content);
      setServerSkillsError(null);
      await loadServerSkills();
    } catch (e) {
      setServerSkillsError(String(e));
    }
  };

  // H4 — delete a server skill (with confirm, like WinForms).
  const removeServerSkill = async (alias: string) => {
    if (!window.confirm(tf(lang, "deleteSkillConfirm", alias))) return;
    try {
      await deleteAgentSkill(liveBase, draft.apiKey, alias);
      setServerSkills((prev) => prev.filter((s) => s.alias !== alias));
    } catch (e) {
      setServerSkillsError(String(e));
    }
  };

  const loadAdmin = async (keyOverride?: string) => {
    const key = keyOverride ?? (await syncApiKey());
    if (!key) {
      setAdminError("API key is empty — set it above first.");
      return;
    }
    setAdminLoading(true);
    setAdminError(null);
    try {
      const [acc, k] = await Promise.all([
        fetchAccounts(liveBase, key),
        fetchKeys(liveBase, key),
      ]);
      setAccounts((unwrapData<GatewayAccount[]>(acc) ?? []) as GatewayAccount[]);
      setKeys((unwrapData<GatewayKey[]>(k) ?? []) as GatewayKey[]);
      setAccountsLoaded(true);
    } catch (e) {
      // Keep the old list (WinForms leaves the list on a failed GET) and
      // report the failure; accountsLoaded stays as it was, so a failed
      // first load never shows "(no accounts yet)".
      setAdminError(tf(lang, "accountsLoadFailed", String(e)));
    } finally {
      setAdminLoading(false);
    }
  };

  // G9 — admin status (fetchAdminStatus existed but was never called).
  const loadAdminStatus = async () => {
    if (!draft.apiKey) {
      setAdminError("API key is empty — set it above first.");
      return;
    }
    try {
      const st = await fetchAdminStatus(liveBase, draft.apiKey);
      setAdminStatus(JSON.stringify(st, null, 2).slice(0, 2000));
      setAdminError(null);
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // F8 — enable/disable one account.
  const toggleAccount = async (a: GatewayAccount) => {
    const enabled = a.enabled !== false;
    try {
      await setAccountEnabled(liveBase, draft.apiKey, a.id, !enabled);
      await loadAdmin();
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // F9 — delete one account (confirm, like WinForms DeleteAccountConfirm).
  const removeAccount = async (a: GatewayAccount) => {
    if (!window.confirm(tf(lang, "deleteAccountConfirm", a.label ?? a.id))) return;
    try {
      await deleteAccount(liveBase, draft.apiKey, a.id);
      await loadAdmin();
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // Account-management port (WinForms PreferencesForm): "Open / restore
  // Muse sign-in" reopens the saved session (MainForm.ConnectAccount,
  // forceLogin, label = last-used account); "Add account" opens a fresh
  // WebView session (MainForm.AddAccount, clearExistingSession, label
  // "Account N"). Both close Settings first — App opens LoginDialog.
  const openLoginRestore = () => {
    props.onOpenLogin?.({ clearSession: false, initialLabel: lastUsedAccountLabel(accounts) });
  };
  const openAddAccount = () => {
    props.onOpenLogin?.({ clearSession: true, initialLabel: tf(lang, "accountNumber", accounts.length + 1) });
  };

  /** Connected snapshot (WinForms "Muse account connected"): only computed
   * from a successfully loaded list — a failed load never claims connected. */
  const connected = accountsLoaded && anySavedLogin(accounts);

  // G8 — quota + renew for one account.
  const showQuota = async (a: GatewayAccount) => {
    try {
      const q = await fetchQuota(liveBase, draft.apiKey, a.id);
      setQuotaInfo((prev) => ({ ...prev, [a.id]: JSON.stringify(q).slice(0, 500) }));
    } catch (e) {
      setAdminError(String(e));
    }
  };

  const renewOneAccount = async (a: GatewayAccount) => {
    if (!window.confirm(tf(lang, "renewConfirm", a.label ?? a.id))) return;
    try {
      await renewAccount(liveBase, draft.apiKey, a.id);
      await loadAdmin();
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // G4 — create a key; plaintext shown once + copied automatically.
  const createKey = async () => {
    if (!draft.apiKey) {
      setAdminError("API key is empty — set it above first.");
      return;
    }
    try {
      const now = new Date();
      const pad = (n: number) => String(n).padStart(2, "0");
      const name = `Key ${pad(now.getHours())}:${pad(now.getMinutes())} ${pad(now.getDate())}/${pad(now.getMonth() + 1)}`;
      const plain = await createApiKey(liveBase, draft.apiKey, name);
      setNewKeyPlain(plain);
      setNewKeyCopied(false);
      try {
        await navigator.clipboard.writeText(plain);
        setNewKeyCopied(true);
      } catch {
        /* clipboard unavailable — the plaintext box below still shows it */
      }
      await loadAdmin();
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // G5 — revoke / restore a key.
  const toggleKeyRevoked = async (k: GatewayKey) => {
    if (!k.id) return;
    try {
      await setKeyRevoked(liveBase, draft.apiKey, k.id, !k.revoked);
      await loadAdmin();
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // G6 — delete a key (confirm, like WinForms DeleteKeyConfirm).
  const removeKey = async (k: GatewayKey) => {
    if (!k.id) return;
    if (!window.confirm(tf(lang, "deleteKeyConfirm", k.name ?? k.label ?? k.id))) return;
    try {
      await deleteApiKey(liveBase, draft.apiKey, k.id);
      await loadAdmin();
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // I13 — manual update check (same lib as the App banner).
  const runUpdateCheck = async () => {
    setUpdateChecking(true);
    setUpdateStatus(t(lang, "updateChecking"));
    try {
      const res = await checkForUpdates(true);
      if (res.status === "available" && res.update && isTrustedUpdateLink(res.update)) {
        setAvailableUpdate(res.update);
        setUpdateStatus(tf(lang, "updateAvailable", res.update.version));
      } else if (res.status === "current") {
        setAvailableUpdate(null);
        setUpdateStatus(t(lang, "updateCurrent"));
      } else if (res.status === "rate-limited") {
        setUpdateStatus(t(lang, "updateRateLimited"));
      } else {
        setUpdateStatus(t(lang, "updateUnavailable"));
      }
    } catch {
      setUpdateStatus(t(lang, "updateUnavailable"));
    } finally {
      setUpdateChecking(false);
    }
  };

  const save = () => {
    props.onSave(draft);
    props.onClose();
  };

  const addSkill = () => {
    const alias = skillAlias.trim();
    if (!alias) return;
    const next = [...skills, { alias, path: skillPath.trim(), note: "" }];
    setSkills(next);
    saveSkills(next);
    setSkillAlias("");
    setSkillPath("");
  };

  const removeSkill = (alias: string) => {
    const next = skills.filter((s) => s.alias !== alias);
    setSkills(next);
    saveSkills(next);
  };

  const openWorkspaceFolder = async () => {
    if (!draft.workspacePath) return;
    try {
      await openPath(draft.workspacePath);
    } catch (e) {
      setAdminError(String(e));
    }
  };

  // I10 — app data folder comes from the Rust sidecar data dir
  // (<exe dir>/Muse2Api/data: accounts, agent sessions, keys).
  const openDataFolder = async () => {
    try {
      const dir = await invoke<string>("app_data_dir");
      await openPath(dir);
    } catch (e) {
      setAdminError(String(e));
    }
  };

  return (
    <Dialog open={props.open} onOpenChange={(o) => { if (!o) props.onClose(); }}>
      <DialogContent className="max-h-[85vh] max-w-xl overflow-y-auto border-[#343541] bg-[#1D1D26] text-[#F5F5F7]">
        <DialogHeader>
          <DialogTitle>{t(lang, "settings")}</DialogTitle>
          <DialogDescription className="text-[#A1A1AA]">
            Muse · v{APP_VERSION} (Tauri) — UI language and reply language are separate.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-5 py-2 text-[13px]">
          {/* Language */}
          <section>
            <h3 className="mb-2 font-semibold">{t(lang, "settingsLanguage")}</h3>
            <div className="flex gap-2">
              {(["en", "id"] as UiLanguage[]).map((l) => (
                <Button
                  key={l}
                  variant={draft.uiLanguage === l ? "default" : "secondary"}
                  className={draft.uiLanguage === l ? "bg-[#3867F5] text-white" : "bg-[#292A34]"}
                  onClick={() => setDraft({ ...draft, uiLanguage: l })}
                >
                  {l === "en" ? "English" : "Bahasa Indonesia"}
                </Button>
              ))}
            </div>
            <label className="mt-3 flex cursor-pointer items-start gap-2">
              <input
                type="checkbox"
                checked={draft.followQuestionLanguage}
                onChange={(e) => setDraft({ ...draft, followQuestionLanguage: e.currentTarget.checked })}
                className="mt-0.5 accent-[#3867F5]"
              />
              <span>
                <span className="font-medium">{t(lang, "settingsFollowQuestion")}</span>
                <span className="block text-[12px] text-[#A1A1AA]">{t(lang, "settingsFollowQuestionDesc")}</span>
              </span>
            </label>
          </section>

          {/* Theme (I2 — Light ported from DesktopTheme LightPalette) */}
          <section>
            <h3 className="mb-2 font-semibold">{t(lang, "settingsTheme")}</h3>
            <div className="flex gap-2">
              {(["dark", "light"] as const).map((mode) => (
                <Button
                  key={mode}
                  variant={draft.theme === mode ? "default" : "secondary"}
                  className={draft.theme === mode ? "bg-[#3867F5] text-white" : "bg-[#292A34]"}
                  onClick={() => setDraft({ ...draft, theme: mode })}
                >
                  {mode === "dark" ? t(lang, "themeDark") : t(lang, "themeLight")}
                </Button>
              ))}
            </div>
            <p className="mt-1 text-[12px] text-[#71717A]">{t(lang, "settingsThemeNote")}</p>
          </section>

          {/* Turbo / approval */}
          <section>
            <h3 className="mb-1 font-semibold">{t(lang, "settingsTurbo")}</h3>
            <p className="mb-2 text-[12px] text-[#A1A1AA]">{t(lang, "settingsTurboDesc")}</p>
            <select
              value={draft.turboMode}
              onChange={(e) => setDraft({ ...draft, turboMode: e.currentTarget.value as TurboMode })}
              className="w-full rounded-md border border-[#343541] bg-[#292A34] px-2 py-2"
            >
              <option value="ask">{t(lang, "turboAsk")}</option>
              <option value="readonly">{t(lang, "turboReadonly")}</option>
              <option value="turbo">{t(lang, "turboTurbo")}</option>
            </select>
          </section>

          {/* Backend */}
          <section>
            <h3 className="mb-1 font-semibold">{t(lang, "driverLabel")}</h3>
            <select
              value={draft.driver}
              onChange={(e) => setDraft({ ...draft, driver: e.currentTarget.value as MuseDriver })}
              className="w-full rounded-md border border-[#343541] bg-[#292A34] px-2 py-2"
            >
              <option value="mock">{t(lang, "driverMock")}</option>
              <option value="browser">{t(lang, "driverBrowser")}</option>
            </select>
            <p className="mt-1 text-[12px] text-[#71717A]">{t(lang, "driverNote")}</p>
            {/* Defect B: the "install it first" clause is a failure hint,
                not a standing note — it only makes sense while the
                backend is actually down. Showing it next to a healthy
                backend is what made Bree read it as "backend fails to
                start" in Settings. */}
            {!props.backendHealthy && (
              <p className="mt-1 text-[12px] text-[#71717A]">{t(lang, "driverInstallNote")}</p>
            )}
          </section>

          {/* Backend URL + keys (G1/G2: visible + Copy) */}
          <section>
            <h3 className="mb-2 font-semibold">{t(lang, "settingsBackendUrl")}</h3>
            <div className="flex gap-1.5">
              <Input
                value={draft.backendBaseUrl}
                onChange={(e) => setDraft({ ...draft, backendBaseUrl: e.currentTarget.value })}
                className="min-w-0 flex-1 border-[#343541] bg-[#292A34] font-mono text-[12px]"
              />
              <Button
                variant="outline"
                size="sm"
                className="shrink-0 border-[#343541]"
                onClick={() => void navigator.clipboard.writeText(draft.backendBaseUrl).catch(() => undefined)}
                title={t(lang, "copy")}
                aria-label={t(lang, "copy")}
              >
                <Copy className="h-3.5 w-3.5" />
              </Button>
            </div>
            <SecretField
              label={t(lang, "settingsApiKey")}
              value={draft.apiKey}
              onChange={(v) => setDraft({ ...draft, apiKey: v })}
              placeholder="sk-… (stored locally only)"
            />
          </section>

          {/* Accounts & keys */}
          <section>
            <div className="mb-2 flex items-center gap-2">
              <h3 className="font-semibold">{t(lang, "settingsAccounts")} / {t(lang, "settingsKeys")}</h3>
              <Button variant="outline" size="sm" className="ml-auto border-[#343541]" onClick={() => void loadAdmin()} disabled={adminLoading}>
                <RefreshCw className={`h-3.5 w-3.5 ${adminLoading ? "animate-spin" : ""}`} /> {t(lang, "refresh")}
              </Button>
            </div>
            {adminError && <p className="mb-2 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-[12px] text-red-300">{adminError}</p>}
            {/* Connected snapshot (WinForms PreferencesForm): only from a
                successfully loaded list — a failed load never claims
                connected, and the empty state below is likewise load-gated. */}
            {accountsLoaded && (
              <p className={`mb-2 text-[12px] ${connected ? "text-emerald-400" : "text-[#A1A1AA]"}`}>
                {t(lang, connected ? "accountConnected" : "accountNotConnected")}
              </p>
            )}
            <div className="mb-2">
              <Button variant="outline" size="sm" className="border-[#343541]" onClick={openLoginRestore} disabled={!props.onOpenLogin}>
                {t(lang, "openLogin")}
              </Button>
            </div>
            <p className="mb-2 text-[12px] text-[#A1A1AA]">{t(lang, "accountsHint")}</p>
            {accountsLoaded && accounts.length === 0 && (
              <p className="text-[12px] text-[#71717A]">{t(lang, "accountsEmpty")}</p>
            )}
            {accounts.length > 0 && (
              <ul className="space-y-1">
                  {accounts.map((a) => (
                    <li key={a.id} className="rounded border border-[#343541] bg-[#292A34] px-2.5 py-1.5 text-[12px]">
                      <div className="flex items-center gap-2">
                        <span className={`h-2 w-2 shrink-0 rounded-full ${a.enabled === false ? "bg-zinc-500" : "bg-emerald-400"}`} />
                        <span className="font-medium">{a.label ?? a.id}</span>
                        <span className="ml-auto font-mono text-[11px] text-[#71717A]">
                          {a.status ?? ""}{a.enabled === false ? t(lang, "accountDisabledSuffix") : ""} · {a.id.slice(0, 12)}
                        </span>
                      </div>
                      <div className="mt-1.5 flex flex-wrap gap-1.5 pl-4">
                        <Button size="sm" variant="outline" className="h-6 border-[#343541] px-2 text-[11px]" onClick={() => void toggleAccount(a)}>
                          {a.enabled === false ? t(lang, "enableAccount") : t(lang, "disableAccount")}
                        </Button>
                        <Button size="sm" variant="outline" className="h-6 border-[#343541] px-2 text-[11px]" onClick={() => void showQuota(a)}>
                          {t(lang, "quota")}
                        </Button>
                        <Button size="sm" variant="outline" className="h-6 border-[#343541] px-2 text-[11px]" onClick={() => void renewOneAccount(a)}>
                          {t(lang, "renew")}
                        </Button>
                        <Button
                          size="sm"
                          variant="outline"
                          className="h-6 border-red-500/40 px-2 text-[11px] text-red-300"
                          onClick={() => void removeAccount(a)}
                          title={t(lang, "deleteAccount")}
                        >
                          <Trash2 className="h-3 w-3" /> {t(lang, "deleteAccount")}
                        </Button>
                      </div>
                      {quotaInfo[a.id] && (
                        <pre className="mt-1.5 max-h-24 overflow-auto whitespace-pre-wrap rounded border border-[#343541] bg-[#17171D] p-2 font-mono text-[11px] text-[#A1A1AA]">
                          {quotaInfo[a.id]}
                        </pre>
                      )}
                    </li>
                  ))}
                </ul>
                )}
                <div className="mt-2 flex gap-1.5">
                  <Button size="sm" className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={openAddAccount} disabled={!props.onOpenLogin}>
                    <Plus className="h-3.5 w-3.5" /> {t(lang, "addAccount")}
                  </Button>
                  <Button size="sm" className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={() => void createKey()} disabled={!draft.apiKey}>
                    <Plus className="h-3.5 w-3.5" /> {t(lang, "createKey")}
                  </Button>
                  <Button size="sm" variant="outline" className="border-[#343541]" onClick={() => void loadAdminStatus()} disabled={!draft.apiKey}>
                    {t(lang, "adminStatus")}
                  </Button>
                </div>
                {newKeyPlain && (
                  <div className="mt-2 rounded border border-emerald-500/40 bg-emerald-500/10 px-2.5 py-2">
                    <p className="text-[12px] font-medium text-emerald-300">
                      {t(lang, "keyCreatedTitle")}{newKeyCopied ? ` — ${t(lang, "keyCopied")}` : ""}
                    </p>
                    <p className="mt-1 break-all font-mono text-[12px] text-emerald-100">{newKeyPlain}</p>
                    <p className="mt-1 text-[11px] text-emerald-200/80">{t(lang, "keyCreatedBody").split("\n\n")[2] ?? ""}</p>
                    <Button
                      size="sm"
                      variant="outline"
                      className="mt-1.5 h-6 border-emerald-500/40 px-2 text-[11px] text-emerald-200"
                      onClick={() => void navigator.clipboard.writeText(newKeyPlain).then(() => setNewKeyCopied(true)).catch(() => undefined)}
                    >
                      <Copy className="h-3 w-3" /> {t(lang, "copyKey")}
                    </Button>
                  </div>
                )}
                {adminStatus && (
                  <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded border border-[#343541] bg-[#17171D] p-2 font-mono text-[11px] text-[#A1A1AA]">
                    {adminStatus}
                  </pre>
                )}
                {accountsLoaded && keys.length === 0 && (
                  <p className="mt-1.5 text-[12px] text-[#71717A]">{t(lang, "noKeys")}</p>
                )}
                <ul className="mt-1.5 space-y-1">
                  {keys.map((k, i) => (
                    <li key={k.id ?? i} className="rounded border border-[#343541] bg-[#292A34] px-2.5 py-1.5 text-[12px]">
                      <div className="flex items-center gap-2">
                        <span className="font-mono text-[#A1A1AA]">
                          {k.name ?? k.label ?? k.id ?? "key"}{k.prefix ? ` — ${k.prefix}…` : ""}{k.revoked ? t(lang, "keyRevokedSuffix") : ""}
                        </span>
                        <span className="ml-auto flex gap-1">
                          <Button size="sm" variant="outline" className="h-6 border-[#343541] px-2 text-[11px]" onClick={() => void toggleKeyRevoked(k)} disabled={!k.id}>
                            {k.revoked ? t(lang, "restoreKey") : t(lang, "revokeKey")}
                          </Button>
                          <Button
                            size="sm"
                            variant="outline"
                            className="h-6 border-red-500/40 px-2 text-[11px] text-red-300"
                            onClick={() => void removeKey(k)}
                            disabled={!k.id}
                            title={t(lang, "deleteKey")}
                          >
                            <Trash2 className="h-3 w-3" />
                          </Button>
                        </span>
                      </div>
                    </li>
                  ))}
                </ul>
          </section>

          {/* Skills (server — used by the agent loop) */}
          <section>
            <div className="mb-2 flex items-center gap-2">
              <h3 className="font-semibold">{t(lang, "settingsServerSkills")}</h3>
              <Button variant="outline" size="sm" className="ml-auto border-[#343541]" onClick={() => void loadServerSkills()} disabled={serverSkillsLoading}>
                <RefreshCw className={`h-3.5 w-3.5 ${serverSkillsLoading ? "animate-spin" : ""}`} /> {t(lang, "refresh")}
              </Button>
              <Button variant="outline" size="sm" className="border-[#343541]" onClick={() => skillFileRef.current?.click()} title={t(lang, "importSkillNote")}>
                <Plus className="h-3.5 w-3.5" /> {t(lang, "importSkill")}
              </Button>
              <input
                ref={skillFileRef}
                type="file"
                accept=".md,text/markdown"
                className="hidden"
                onChange={(e) => {
                  const f = e.currentTarget.files?.[0];
                  if (f) void importSkillFile(f);
                  e.currentTarget.value = "";
                }}
              />
            </div>
            <p className="mb-2 text-[12px] text-[#A1A1AA]">{t(lang, "settingsSkillsNote")}</p>
            {serverSkillsError && <p className="mb-2 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-[12px] text-red-300">{serverSkillsError}</p>}
            {serverSkills.length === 0 && !serverSkillsError ? (
              <p className="text-[12px] text-[#71717A]">{t(lang, "settingsServerSkillsEmpty")}</p>
            ) : (
              <ul className="space-y-1">
                {serverSkills.map((s) => (
                  <li key={s.alias} className="flex items-center gap-2 rounded border border-[#343541] bg-[#292A34] px-2.5 py-1.5 text-[12px]">
                    <input
                      type="checkbox"
                      checked={s.enabled}
                      onChange={(e) => void toggleServerSkill(s.alias, e.currentTarget.checked)}
                      className="accent-[#3867F5]"
                      title={s.enabled ? "Enabled" : "Disabled"}
                    />
                    <span className="font-mono font-medium text-[#8aa8ff]">@{s.alias}</span>
                    <span className="min-w-0 flex-1 truncate text-[#A1A1AA]">{s.description || s.name}</span>
                    <button onClick={() => void removeServerSkill(s.alias)} className="text-[#71717A] hover:text-red-400" title={t(lang, "deleteSkill")}>
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {/* Skills (local) */}
          <section>
            <h3 className="mb-1 font-semibold">{t(lang, "settingsSkills")}</h3>
            <div className="flex gap-1.5">
              <Input value={skillAlias} onChange={(e) => setSkillAlias(e.currentTarget.value)} placeholder={t(lang, "skillAlias")} className="border-[#343541] bg-[#292A34]" />
              <Input value={skillPath} onChange={(e) => setSkillPath(e.currentTarget.value)} placeholder={t(lang, "skillPath")} className="border-[#343541] bg-[#292A34]" />
              <Button className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={addSkill}>
                <Plus className="h-4 w-4" /> {t(lang, "add")}
              </Button>
            </div>
            <ul className="mt-2 space-y-1">
              {skills.map((s) => (
                <li key={s.alias} className="flex items-center gap-2 rounded border border-[#343541] bg-[#292A34] px-2.5 py-1.5 text-[12px]">
                  <span className="font-mono font-medium text-[#8aa8ff]">{s.alias}</span>
                  <span className="min-w-0 flex-1 truncate text-[#A1A1AA]">{s.path}</span>
                  <button onClick={() => removeSkill(s.alias)} className="text-[#71717A] hover:text-red-400" title={t(lang, "remove")}>
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          </section>

          {/* Updates (I13) */}
          <section>
            <h3 className="mb-1 font-semibold">{t(lang, "updateSection")}</h3>
            <p className="mb-2 text-[12px] text-[#A1A1AA]">
              Muse CLI v{APP_VERSION} · {t(lang, "updateNote")}
            </p>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" className="border-[#343541]" onClick={() => void runUpdateCheck()} disabled={updateChecking}>
                <RefreshCw className={`h-3.5 w-3.5 ${updateChecking ? "animate-spin" : ""}`} /> {t(lang, "updateCheck")}
              </Button>
              {availableUpdate && isTrustedUpdateLink(availableUpdate) && (
                <>
                  <Button
                    size="sm"
                    className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]"
                    onClick={() => void openUrl(availableUpdate.url)}
                  >
                    {t(lang, "viewUpdate")}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    className="border-[#343541]"
                    onClick={() => {
                      dismissUpdate(availableUpdate.version);
                      setAvailableUpdate(null);
                      setUpdateStatus(null);
                    }}
                  >
                    {t(lang, "dismissUpdate")}
                  </Button>
                </>
              )}
            </div>
            {updateStatus && <p className="mt-1.5 text-[12px] text-[#A1A1AA]">{updateStatus}</p>}
          </section>

          {/* Advanced (I10–I12) */}
          <section>
            <button
              onClick={() => setAdvancedOpen(!advancedOpen)}
              className="flex w-full items-center gap-2 text-left font-semibold hover:text-white"
              aria-expanded={advancedOpen}
            >
              {advancedOpen ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
              {advancedOpen ? t(lang, "collapseAdvanced") : t(lang, "advanced")}
            </button>
            {advancedOpen && (
              <div className="mt-2 space-y-3">
                <div>
                  <label className="mb-1 block text-[12px] text-[#A1A1AA]">{t(lang, "terminal")}</label>
                  <select
                    value={draft.shell || ""}
                    onChange={(e) => setDraft({ ...draft, shell: e.currentTarget.value })}
                    className="w-full rounded-md border border-[#343541] bg-[#292A34] px-2 py-2"
                  >
                    <option value="">(default)</option>
                    {["cmd", "powershell", "pwsh", "sh", "bash", "zsh"].map((s) => (
                      <option key={s} value={s}>{s}</option>
                    ))}
                  </select>
                </div>
                <div>
                  <div className="flex flex-wrap gap-2">
                    <Button variant="outline" size="sm" className="border-[#343541]" onClick={() => void openWorkspaceFolder()} disabled={!draft.workspacePath}>
                      {t(lang, "openWorkspaceFolder")}
                    </Button>
                    <Button variant="outline" size="sm" className="border-[#343541]" onClick={() => void openDataFolder()}>
                      {t(lang, "openAppDataFolder")}
                    </Button>
                  </div>
                  <p className="mt-1 text-[12px] text-[#71717A]">{t(lang, "openDataFolderNote")}</p>
                </div>
                <div>
                  <label className="mb-1 block text-[12px] text-[#A1A1AA]">{t(lang, "diagnosticsLog")}</label>
                  <textarea
                    readOnly
                    value={props.diagnostics || t(lang, "noSetupErrors")}
                    rows={6}
                    spellCheck={false}
                    className="w-full rounded-md border border-[#343541] bg-[#17171D] p-2.5 font-mono text-[11px] leading-relaxed text-[#A1A1AA] outline-none"
                  />
                  {/* Defect B: never render a stale failure next to a
                      healthy backend — App already nulls backendError
                      when healthy; this is the second guard. */}
                  {props.backendError && !props.backendHealthy && (
                    <p className="mt-1 text-[11px] text-red-400">{props.backendError}</p>
                  )}
                </div>
              </div>
            )}
          </section>

          <div className="flex justify-end gap-2 pt-1">
            <Button variant="secondary" className="bg-[#292A34]" onClick={props.onClose}>{t(lang, "cancel")}</Button>
            <Button className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]" onClick={save}>{t(lang, "save")}</Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
