// Login dialog — in-app sign-in only (mirrors the WinForms LoginForm).
//
// The "Sign in inside the app" button calls the Tauri `start_login`
// command, which launches `muse-gateway login` — a WebView2 sign-in window
// with its own profile (mirrors the WinForms LoginForm). The user signs in
// there; the gateway saves the account itself via POST /admin/accounts.
// This app never sees the password, and the user's own browser is not
// touched. There is no manual cookie paste here, same as muse.exe: if the
// in-app window cannot sign in, there is no other way in from this dialog.
//
// "Other account" opens the same window with a cleared WebView session
// (`--clear-session`, LoginForm.cs `clearExistingSession`), so a different
// Muse account can sign in and be saved as a new account.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { LogIn } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { BackendStatus, GatewayAccount, Settings } from "@/lib/types";
import { fetchAccounts, unwrapData } from "@/lib/gateway";
import { t, tf, type Lang } from "@/lib/i18n";

interface Props {
  lang: Lang;
  open: boolean;
  onClose: () => void;
  settings: Settings;
  backend: BackendStatus;
  /** Label the sign-in is saved under, when the caller pre-fills it
   * (Settings "Add account" passes "Account N"; default "Personal").
   * The field stays editable — Bree can still rename it here. */
  initialLabel?: string;
  /** Open in "add account" mode (Settings "Add account", WinForms
   * MainForm.AddAccount): the sign-in window opens with a cleared WebView
   * session, like the "Other account" button below. Default false: the
   * saved session is restored (MainForm.ConnectAccount). */
  clearSession?: boolean;
  /** Why the dialog was opened (send gate in App.tsx): shown as a banner
   * so the user knows signing in is what unblocks their chat. Null when
   * the dialog was opened from the sidebar / Settings as usual. */
  notice?: string | null;
}

interface LoginStatus {
  launched: boolean;
  already_open: boolean;
  pid?: number | null;
}

export function LoginDialog(props: Props) {
  const { lang } = props;
  const [label, setLabel] = useState("Personal");
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loginWindowOpen, setLoginWindowOpen] = useState(false);
  /** Saved accounts from GET /admin/accounts, loaded each time the dialog
   * opens (Cacat A, 3 Nov 2026): the dialog must SAY which mode it is in
   * and which accounts already exist. Null = not loaded yet / load
   * failed — the dialog then falls back to the old UI instead of
   * claiming anything about the accounts. */
  const [accounts, setAccounts] = useState<GatewayAccount[] | null>(null);
  const pollRef = useRef<number | null>(null);
  /** Auto-close timer (defect 1): armed only on a successful sign-in, so
   * the dialog closes ~1s after the green banner. Stored in a ref so it
   * survives re-renders and can be cancelled from any path below. */
  const autoCloseTimerRef = useRef<number | null>(null);
  /** Latest onClose + open flag, so the timer (created inside the poll
   * interval closure) always calls the current close path — the same
   * onClose the Cancel button uses, so App's refreshAccounts() runs. */
  const onCloseRef = useRef(props.onClose);
  onCloseRef.current = props.onClose;
  const openRef = useRef(props.open);
  openRef.current = props.open;

  const clearAutoClose = () => {
    if (autoCloseTimerRef.current !== null) {
      window.clearTimeout(autoCloseTimerRef.current);
      autoCloseTimerRef.current = null;
    }
  };

  /** Arm the ~1s auto-close after a successful sign-in. Only success
   * arms it — a failed check, no account, or a window closed without a
   * save leaves the dialog open so Bree can retry. Cancelled if the
   * dialog is closed by hand, a new sign-in starts, or the component
   * unmounts (see the reset effect, startInAppLogin, and the unmount
   * cleanup). */
  const scheduleAutoClose = () => {
    clearAutoClose();
    autoCloseTimerRef.current = window.setTimeout(() => {
      autoCloseTimerRef.current = null;
      // Guard: Bree may have closed the dialog by hand during the 1s
      // window — never call onClose on an already-closed dialog.
      if (openRef.current) onCloseRef.current();
    }, 1000);
  };

  /** Label submitted with the sign-in, for naming the saved account in the
   * success message (best effort — see the poll completion below). */
  const submittedLabelRef = useRef("Personal");

  const baseUrl = props.backend.base_url ?? props.settings.backendBaseUrl;

  // Fresh dialog each time it opens (WinForms constructs a new LoginForm
  // per sign-in): label from the caller, or "Personal" by default.
  // Also reload the saved accounts so the mode banner below reflects
  // the gateway's current list (Cacat A).
  useEffect(() => {
    if (props.open) {
      setLabel(props.initialLabel ?? "Personal");
      setStatus(null);
      setError(null);
      setAccounts(null);
      void (async () => {
        try {
          const res = await fetchAccounts(baseUrl, props.settings.apiKey);
          setAccounts(unwrapData<GatewayAccount[]>(res) ?? []);
        } catch {
          // Backend down / unreachable: leave accounts null — the dialog
          // still works, it just cannot show the mode banner.
          setAccounts(null);
        }
      })();
    } else {
      // Manual close (Cancel / X / Escape / click outside) cancels a
      // pending auto-close so the timer can never fire on a closed dialog.
      clearAutoClose();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.open]);

  const stopPolling = () => {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
  };

  useEffect(() => () => {
    stopPolling();
    clearAutoClose();
  }, []);

  // While the in-app sign-in window is open, poll until it closes, then
  // check how many accounts the gateway now holds.
  useEffect(() => {
    if (!loginWindowOpen) return;
    pollRef.current = window.setInterval(() => {
      void (async () => {
        try {
          const s = await invoke<LoginStatus>("login_status");
          if (s.already_open) return;
          stopPolling();
          setLoginWindowOpen(false);
          setStatus(t(lang, "loginCheckAccounts"));
          try {
            const res = await fetchAccounts(baseUrl, props.settings.apiKey);
            const list = unwrapData<unknown[]>(res) ?? [];
            // Name the account that was just saved when it can be pinned
            // down: exactly one account carries the submitted label.
            // Duplicate labels (or none) stay ambiguous — keep the old
            // total-count message rather than guessing.
            const wanted = submittedLabelRef.current;
            const matches = list.filter(
              (a) => (a as { label?: unknown } | null)?.label === wanted,
            );
            if (matches.length === 1) {
              // Success, account pinned down by label: show the green
              // banner, then auto-close ~1s later (defect 1).
              setStatus(tf(lang, "loginSavedAccount", wanted));
              scheduleAutoClose();
            } else if (list.length > 0) {
              // Success, account not pinnable (duplicate / missing label):
              // same green banner with the total count, then auto-close.
              setStatus(
                `${t(lang, "loginDone")} (${list.length} ${list.length === 1 ? "account" : "accounts"})`,
              );
              scheduleAutoClose();
            } else {
              // No account was saved — do NOT auto-close: Bree must see
              // this and be able to retry from the still-open dialog.
              setStatus(t(lang, "loginNoAccount"));
            }
          } catch (e) {
            // The window closed; the account check itself failed (e.g. the
            // backend was stopped meanwhile). Report the check failure
            // plainly instead of claiming success or failure of the login.
            setStatus(`${t(lang, "loginWindowClosed")} (${String(e)})`);
          }
        } catch {
          // login_status itself failed — stop polling quietly; the user can
          // retry from the dialog.
          stopPolling();
          setLoginWindowOpen(false);
        }
      })();
    }, 2000);
    return stopPolling;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loginWindowOpen]);

  // `clearSession` (dialog "Other account") opens the window with a
  // cleared WebView session, so a different Muse account can sign in
  // instead of restoring the saved one.
  //
  // TAURI ARG-NAME TRAP (3 Nov 2026, root cause of 7x `clear_session=false`
  // in login.log): Tauri v2 commands take camelCase args by default
  // (tauri-macros `argument_case` defaults to Camel; the live exe's
  // command metadata lists `clearSession`). The invoke below MUST use
  // `clearSession` — a snake_case `clear_session` key is silently
  // ignored by serde (unknown field), `clearSession` stays missing,
  // `Option<bool>` defaults to None → false, and the child launches
  // WITHOUT --clear-session, restoring the saved session instead of
  // showing a fresh sign-in. Every other invoke in this app already
  // uses camelCase; this was the only snake_case one.
  const startInAppLogin = async (clearSession = false) => {
    // Normal mode + a label that matches NO saved account would only
    // RENAME the restored account (dedup rule) — never silently do
    // that: confirm explicitly first. Add mode (clearSession true) and
    // a matching label skip this; unknown account list (null) also
    // skips it — the dialog cannot know what it would rename.
    if (!clearSession && accounts !== null && accounts.length > 0) {
      const wanted = label.trim() || "Personal";
      const known = accounts.map((a) => (a.label ?? "").trim() || a.id);
      if (!known.includes(wanted)) {
        if (!window.confirm(tf(lang, "loginRenameConfirm", wanted, known.join(", ")))) {
          return;
        }
      }
    }
    // A new sign-in cancels any pending auto-close from a previous
    // success banner — the dialog must stay open for this attempt.
    clearAutoClose();
    setError(null);
    setStatus(null);
    setBusy(true);
    submittedLabelRef.current = label.trim() || "Personal";
    try {
      const res = await invoke<LoginStatus>("start_login", {
        label: label.trim() || "Personal",
        driver: props.settings.driver,
        clearSession,
      });
      if (res.already_open) {
        setStatus(t(lang, "loginAlreadyOpen"));
        setLoginWindowOpen(true);
      } else {
        setStatus(t(lang, "loginWindowOpened"));
        setLoginWindowOpen(true);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Cacat A: what the dialog is about to do, in plain words. In RESTORE
  // mode (sidebar icon / send gate) the main button reuses the saved
  // session — a label that matches no saved account only RENAMES the
  // restored account (dedup rule, same as muse.exe); it never adds a
  // second account. The mismatch warning + prominent "Add another
  // account" button steer Bree to the cleared-session path instead.
  const existingLabels = (accounts ?? []).map((a) => (a.label ?? "").trim() || a.id);
  const labelTrimmed = label.trim();
  const labelMatchesExisting =
    existingLabels.length === 0 || existingLabels.includes(labelTrimmed);
  const showModeBanner =
    props.clearSession || (accounts !== null && accounts.length > 0);

  return (
    <Dialog open={props.open} onOpenChange={(o) => { if (!o) props.onClose(); }}>
      <DialogContent className="max-w-lg border-[#343541] bg-[#1D1D26] text-[#F5F5F7]">
        <DialogHeader>
          <DialogTitle>{t(lang, "loginTitle")}</DialogTitle>
          <DialogDescription className="text-[#A1A1AA]">{t(lang, "loginHeadingDesc")}</DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-2 text-[13px]">
          {props.notice && (
            <div className="rounded border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-200">
              {props.notice}
            </div>
          )}
          <div>
            <p className="mb-1.5 font-medium">{t(lang, "loginInApp")}</p>
            <p className="mb-2 text-[12px] text-[#71717A]">{t(lang, "loginInAppDesc")}</p>
            {showModeBanner && (
              <div className="mb-2 rounded border border-sky-500/40 bg-sky-500/10 px-2.5 py-2 text-[12px] text-sky-200">
                <p className="font-medium">
                  {props.clearSession ? t(lang, "loginModeAdd") : t(lang, "loginModeRestore")}
                </p>
                {!props.clearSession && accounts !== null && accounts.length > 0 && (
                  <p className="mt-1 text-sky-300/90">
                    {tf(lang, "loginExistingAccounts", existingLabels.join(", "))}
                  </p>
                )}
                <p className="mt-1 text-sky-300/90">{t(lang, "loginSecondAccountNote")}</p>
              </div>
            )}
            {!props.clearSession && accounts !== null && accounts.length > 0 && labelTrimmed !== "" && !labelMatchesExisting && (
              <div className="mb-2 rounded border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-200">
                {tf(lang, "loginLabelMismatch", labelTrimmed, existingLabels.join(", "))}
              </div>
            )}
            <label className="mb-1 block text-[12px] text-[#A1A1AA]">{t(lang, "loginCookieLabel")}</label>
            <Input
              value={label}
              onChange={(e) => setLabel(e.currentTarget.value)}
              className="mb-2 border-[#343541] bg-[#292A34]"
            />
            <Button
              className="bg-[#3867F5] text-white hover:bg-[#2f5ae0]"
              disabled={busy || loginWindowOpen}
              onClick={() => void startInAppLogin(props.clearSession ?? false)}
            >
              <LogIn className="h-4 w-4" /> {t(lang, "loginInApp")}
            </Button>
            {props.clearSession ? (
              // Add-account mode: the main button already opens with a
              // cleared session, so "Other account" would be the same
              // action — say so instead of showing a duplicate button.
              <p className="mt-2 text-[12px] text-[#71717A]">{t(lang, "loginOtherAccountNote")}</p>
            ) : accounts !== null && accounts.length > 0 ? (
              // Cacat A: Bree's real path had the add-account action as a
              // small outline button next to the main one — easy to miss,
              // so he used the main button and renamed the old account.
              // With accounts saved, the cleared-session path is a
              // full-width primary button instead.
              <div className="mt-2">
                <Button
                  className="w-full bg-emerald-600 text-white hover:bg-emerald-500"
                  disabled={busy || loginWindowOpen}
                  onClick={() => {
                    setStatus(t(lang, "loginOtherAccountNote"));
                    void startInAppLogin(true);
                  }}
                  title={t(lang, "loginOtherAccountNote")}
                >
                  {t(lang, "loginAddAccountCta")}
                </Button>
                <p className="mt-1.5 text-[12px] text-[#71717A]">{t(lang, "loginOtherAccountNote")}</p>
              </div>
            ) : (
              <Button
                variant="outline"
                className="ml-2 border-[#343541] bg-[#292A34] text-[#F5F5F7]"
                disabled={busy || loginWindowOpen}
                onClick={() => {
                  // "Other account": open the sign-in window with a cleared
                  // WebView session (--clear-session), so a different Muse
                  // account can sign in and be saved as a new account
                  // (same label field above — give the second account its
                  // own label). The saved gateway accounts are untouched.
                  setStatus(t(lang, "loginOtherAccountNote"));
                  void startInAppLogin(true);
                }}
                title={t(lang, "loginOtherAccountNote")}
              >
                {t(lang, "loginOtherAccount")}
              </Button>
            )}
            {loginWindowOpen && (
              <p className="mt-1.5 text-[12px] text-[#A1A1AA]">{t(lang, "loginWindowOpened")}</p>
            )}
          </div>

          {error && (
            <div className="rounded border border-red-500/40 bg-red-500/10 px-2.5 py-2">
              <p className="text-[12px] text-red-300">{error}</p>
              {/* F6 — Retry: reopen the sign-in window after a failure. */}
              <Button
                size="sm"
                variant="outline"
                className="mt-2 border-red-500/40 text-red-200"
                disabled={busy || loginWindowOpen}
                onClick={() => void startInAppLogin(props.clearSession ?? false)}
              >
                {t(lang, "loginRetry")}
              </Button>
            </div>
          )}
          {status && <p className="rounded border border-emerald-500/40 bg-emerald-500/10 px-2.5 py-2 text-[12px] text-emerald-300">{status}</p>}

          <div className="flex justify-end gap-2">
            <Button variant="secondary" className="bg-[#292A34]" onClick={props.onClose}>{t(lang, "cancel")}</Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
