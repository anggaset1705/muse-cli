// Gateway client — talks HTTP to the Rust muse-gateway on loopback.
// Only endpoints implemented in muse-gateway/src/routes.rs
// are used here. Auth: `Authorization: Bearer <key>` (or X-Api-Key).
// No secret is ever hardcoded; keys come from Settings (localStorage).

import type { GatewayAccount } from "./types";

export interface GatewayError {
  status: number;
  message: string;
}

function authHeaders(apiKey?: string): Record<string, string> {
  const h: Record<string, string> = { "Content-Type": "application/json" };
  if (apiKey) h["Authorization"] = `Bearer ${apiKey}`;
  return h;
}

async function readError(res: Response): Promise<string> {
  try {
    const text = await res.text();
    return text.slice(0, 300) || `HTTP ${res.status}`;
  } catch {
    return `HTTP ${res.status}`;
  }
}

export async function gatewayHealth(baseUrl: string): Promise<{ ok: boolean; ready: boolean }> {
  const out = { ok: false, ready: false };
  try {
    const r = await fetch(`${baseUrl}/healthz`, { signal: AbortSignal.timeout(5000) });
    out.ok = r.ok;
  } catch {
    /* unreachable */
  }
  try {
    // /readyz may be slow (~1 min) while Chromium starts under the browser
    // driver, but it must not hang forever and pile up poller requests.
    const r = await fetch(`${baseUrl}/readyz`, { signal: AbortSignal.timeout(30000) });
    out.ready = r.ok;
  } catch {
    /* unreachable */
  }
  return out;
}

export async function fetchModels(baseUrl: string, apiKey: string): Promise<string[]> {
  const res = await fetch(`${baseUrl}/v1/models`, {
    headers: authHeaders(apiKey),
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) throw new Error(`GET /v1/models -> ${res.status}: ${await readError(res)}`);
  const body = await res.json();
  const data = Array.isArray(body?.data) ? body.data : [];
  const ids = data
    .map((m: { id?: string }) => m?.id)
    .filter((id: unknown): id is string => typeof id === "string" && id.length > 0);
  return ids.length > 0 ? ids : ["muse-chat"];
}

export interface ChatTurn {
  role: "user" | "assistant" | "system";
  content: string;
}

export interface StreamCallbacks {
  onDelta: (text: string) => void;
  onDone: (fullText: string, usage?: { in: number; out: number }) => void;
  onError: (message: string) => void;
}

/**
 * POST /v1/chat/completions with stream:true; parses SSE `data:` lines.
 * Sends _muse_reply_language per message and user=<conversationId>.
 * _muse_local_tools is intentionally FALSE here: this is the LEGACY
 * plain-chat path (fallback when /v1/agent/* is unavailable). The agent
 * loop path below (streamAgentTurn) drives the planner protocol through
 * /v1/agent/sessions/{id}/chat instead.
 */
export async function streamChatCompletion(
  baseUrl: string,
  apiKey: string,
  opts: {
    model: string;
    messages: ChatTurn[];
    conversationId: string;
    replyLanguage: "Bahasa Indonesia" | "English";
  },
  cb: StreamCallbacks,
): Promise<void> {
  let res: Response;
  try {
    res = await fetch(`${baseUrl}/v1/chat/completions`, {
      method: "POST",
      headers: authHeaders(apiKey),
      body: JSON.stringify({
        model: opts.model,
        messages: opts.messages,
        stream: true,
        user: opts.conversationId,
        _muse_reply_language: opts.replyLanguage,
        _muse_local_tools: false,
      }),
    });
  } catch (e) {
    cb.onError(`Cannot reach gateway at ${baseUrl}: ${String(e)}`);
    return;
  }
  if (!res.ok) {
    cb.onError(`Gateway ${res.status}: ${await readError(res)}`);
    return;
  }
  if (!res.body) {
    cb.onError("Gateway returned no stream body.");
    return;
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  let full = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const parts = buf.split("\n\n");
      buf = parts.pop() ?? "";
      for (const part of parts) {
        for (const line of part.split("\n")) {
          const trimmed = line.trim();
          if (!trimmed.startsWith("data:")) continue;
          const payload = trimmed.slice(5).trim();
          if (payload === "[DONE]") continue;
          try {
            const json = JSON.parse(payload);
            const delta: string =
              json?.choices?.[0]?.delta?.content ?? json?.choices?.[0]?.message?.content ?? "";
            if (delta) {
              full += delta;
              cb.onDelta(full);
            }
            if (json?.usage) {
              cb.onDone(full, {
                in: json.usage.prompt_tokens ?? 0,
                out: json.usage.completion_tokens ?? 0,
              });
              return;
            }
          } catch {
            // A non-JSON SSE line (e.g. heartbeat) — ignore.
          }
        }
      }
    }
  } catch (e) {
    cb.onError(`Stream interrupted: ${String(e)}`);
    return;
  }
  cb.onDone(full);
}

async function adminGet<T>(baseUrl: string, adminKey: string, path: string): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, { headers: authHeaders(adminKey) });
  if (!res.ok) throw new Error(`${path} -> ${res.status}: ${await readError(res)}`);
  return (await res.json()) as T;
}

export function fetchAdminStatus(baseUrl: string, adminKey: string) {
  return adminGet<Record<string, unknown>>(baseUrl, adminKey, "/admin/status");
}

export function fetchAccounts(baseUrl: string, adminKey: string) {
  return adminGet<unknown>(baseUrl, adminKey, "/admin/accounts");
}

export function fetchKeys(baseUrl: string, adminKey: string) {
  return adminGet<unknown>(baseUrl, adminKey, "/admin/keys");
}

// --- Account "connected" snapshot (WinForms PreferencesForm + MainForm) ---
// WinForms shows "Muse account connected" when the last setup left a saved
// login: `DesktopProfile.HasSavedLogin` = Enabled && Status != Invalid &&
// both session cookies present (`Upstream.MissingSessionCookies` == 0).
// This is a SNAPSHOT of the stored accounts, NOT a live check — same as
// WinForms, where the line comes from the setup result, not a probe.
//
// `Account.Public()` (muse-store account.rs) returns the cookies MASKED
// ("abcd...7890" for long values, "***" for ≤ 8 chars), so a present cookie
// is recognised by a non-empty value that is not "***": "***" is ambiguous
// (empty cookie, or a ≤ 8-char one) and session cookies are long tokens, so
// it is treated as missing rather than claiming a connection. When the
// response carries no `cookies` object at all (older gateway), fall back to
// `enabled && status === 'active'` and document it — the fallback is the
// documented approximation, not the full HasSavedLogin rule.
export const SESSION_COOKIE_NAMES = ["hatch_sess", "hatch_native_auth_device"] as const;

function statusString(a: GatewayAccount): string {
  const st = a.status;
  if (typeof st === "number") return st === 0 ? "active" : st === 1 ? "cooling" : "invalid";
  return String(st ?? "").toLowerCase();
}

function hasSessionCookies(a: GatewayAccount): boolean {
  const cookies = a.cookies;
  if (!cookies || typeof cookies !== "object") return false;
  return SESSION_COOKIE_NAMES.every((name) => {
    const v = (cookies as Record<string, unknown>)[name];
    return typeof v === "string" && v.length > 0 && v !== "***";
  });
}

/** `DesktopProfile.HasSavedLogin` for one account from `GET /admin/accounts`. */
export function hasSavedLogin(a: GatewayAccount): boolean {
  if (a.enabled === false) return false;
  const status = statusString(a);
  if (status === "invalid") return false;
  // No `cookies` field in the response: documented fallback (see above).
  if (a.cookies === undefined) return status === "active";
  return hasSessionCookies(a);
}

/** True when at least one account has a saved login (the Settings status line). */
export function anySavedLogin(accounts: GatewayAccount[]): boolean {
  return accounts.some(hasSavedLogin);
}

/** Label of the most recently used account (last_used_at, then created_at),
 * for prefilling the sign-in dialog; "Personal" when there is no account. */
export function lastUsedAccountLabel(accounts: GatewayAccount[]): string {
  if (accounts.length === 0) return "Personal";
  let best = accounts[0];
  for (const a of accounts) {
    const la = a.last_used_at ?? 0;
    const lb = best.last_used_at ?? 0;
    if (la > lb || (la === lb && (a.created_at ?? 0) > (best.created_at ?? 0))) best = a;
  }
  return best.label?.trim() || "Personal";
}

// --- Admin writes (WinForms Client.cs:98-140) ---------------------------
// PATCH/DELETE /admin/accounts/{id}, POST/PATCH/DELETE /admin/keys,
// GET /admin/accounts/{id}/quota, POST /admin/accounts/{id}/renew.
// Auth is the admin key (same header as the admin GETs above).

async function adminWrite<T>(baseUrl: string, adminKey: string, path: string, method: string, body?: unknown): Promise<T> {
  const res = await fetch(`${baseUrl}${path}`, {
    method,
    headers: authHeaders(adminKey),
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status}: ${await readError(res)}`);
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  if (!text) return undefined as T;
  try {
    return JSON.parse(text) as T;
  } catch {
    return undefined as T;
  }
}

/** F8 — enable/disable one account (WinForms ToggleAccountAsync). */
export function setAccountEnabled(baseUrl: string, adminKey: string, accountId: string, enabled: boolean) {
  return adminWrite<unknown>(baseUrl, adminKey, `/admin/accounts/${encodeURIComponent(accountId)}`, "PATCH", { enabled });
}

/** F9 — delete one account (WinForms DeleteAccountAsync). */
export function deleteAccount(baseUrl: string, adminKey: string, accountId: string) {
  return adminWrite<unknown>(baseUrl, adminKey, `/admin/accounts/${encodeURIComponent(accountId)}`, "DELETE");
}

/** G4 — create an API key; plaintext is returned ONCE (body.api_key). */
export async function createApiKey(baseUrl: string, adminKey: string, name: string): Promise<string> {
  const body = await adminWrite<{ api_key?: string; key?: string }>(baseUrl, adminKey, "/admin/keys", "POST", { name });
  const plain = body?.api_key ?? body?.key;
  if (!plain) throw new Error("Gateway did not return the new key (expected api_key in response).");
  return plain;
}

/** G5 — revoke (true) or restore (false) a key. */
export function setKeyRevoked(baseUrl: string, adminKey: string, keyId: string, revoked: boolean) {
  return adminWrite<unknown>(baseUrl, adminKey, `/admin/keys/${encodeURIComponent(keyId)}`, "PATCH", { revoked });
}

/** G6 — delete a key. */
export function deleteApiKey(baseUrl: string, adminKey: string, keyId: string) {
  return adminWrite<unknown>(baseUrl, adminKey, `/admin/keys/${encodeURIComponent(keyId)}`, "DELETE");
}

/** G8 — quota for one account (no UI in WinForms either; shown here as a bonus). */
export function fetchQuota(baseUrl: string, adminKey: string, accountId: string) {
  return adminGet<unknown>(baseUrl, adminKey, `/admin/accounts/${encodeURIComponent(accountId)}/quota`);
}

/** G8 — renew one account. */
export function renewAccount(baseUrl: string, adminKey: string, accountId: string) {
  return adminWrite<unknown>(baseUrl, adminKey, `/admin/accounts/${encodeURIComponent(accountId)}/renew`, "POST", {});
}

export async function addAccountWithCookies(
  baseUrl: string,
  adminKey: string,
  label: string,
  cookies: Array<{ name: string; value: string; expirationDate?: number }>,
): Promise<unknown> {
  const res = await fetch(`${baseUrl}/admin/accounts`, {
    method: "POST",
    headers: authHeaders(adminKey),
    body: JSON.stringify({ label, cookies, reuse_session: true }),
  });
  if (!res.ok) throw new Error(`POST /admin/accounts -> ${res.status}: ${await readError(res)}`);
  return await res.json();
}

export function unwrapData<T>(body: unknown): T | undefined {
  if (body && typeof body === "object" && "data" in (body as Record<string, unknown>)) {
    return (body as { data?: T }).data;
  }
  return body as T;
}

// ---------------------------------------------------------------------------
// Agent loop client — /v1/agent/* (see muse-gateway/src/routes.rs).
// One AgentSession per conversation, workspace-scoped on the server.
// Auth is the same Bearer key as the other gateway endpoints.
// ---------------------------------------------------------------------------

import type {
  AgentChange,
  AgentPreferences,
  AgentScreenAuthorize,
  AgentScreenPreview,
  AgentScreenStatus,
  AgentScreenWindows,
  AgentSessionView,
  AgentSkill,
  AgentTurnEvent,
  DesktopWindow,
} from "@/lib/types";

async function agentFetch<T>(baseUrl: string, apiKey: string, path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${baseUrl}${path}`, {
      ...init,
      headers: { ...authHeaders(apiKey), ...(init?.headers ?? {}) },
    });
  } catch (e) {
    throw new Error(`Cannot reach gateway at ${baseUrl}: ${String(e)}`);
  }
  if (!res.ok) {
    const body = await readError(res);
    const err = new Error(`${path} -> ${res.status}: ${body}`) as Error & { status?: number };
    err.status = res.status;
    throw err;
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

export function createAgentSession(
  baseUrl: string,
  apiKey: string,
  opts: { workspace: string; shell?: string; model?: string; replyLanguage?: string; turbo?: boolean; maxSteps?: number },
): Promise<AgentSessionView> {
  return agentFetch<AgentSessionView>(baseUrl, apiKey, "/v1/agent/sessions", {
    method: "POST",
    body: JSON.stringify({
      workspace: opts.workspace,
      shell: opts.shell || undefined,
      model: opts.model || undefined,
      reply_language: opts.replyLanguage || undefined,
      turbo: opts.turbo,
      max_steps: opts.maxSteps,
    }),
  });
}

export function getAgentSession(baseUrl: string, apiKey: string, sessionId: string): Promise<AgentSessionView> {
  return agentFetch<AgentSessionView>(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}`);
}

export function deleteAgentSession(baseUrl: string, apiKey: string, sessionId: string): Promise<unknown> {
  return agentFetch<unknown>(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}`, { method: "DELETE" });
}

export function cancelAgentTurn(baseUrl: string, apiKey: string, sessionId: string): Promise<{ cancelled: boolean }> {
  return agentFetch<{ cancelled: boolean }>(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}/cancel`, { method: "POST" });
}

export function getAgentChanges(baseUrl: string, apiKey: string, sessionId: string): Promise<{ session_id: string; changes: AgentChange[] }> {
  return agentFetch(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}/changes`);
}

export function revertAgentChange(baseUrl: string, apiKey: string, sessionId: string, path: string): Promise<{ reverted: boolean; path: string }> {
  return agentFetch(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}/revert`, {
    method: "POST",
    body: JSON.stringify({ path }),
  });
}

export function runAgentTool(
  baseUrl: string,
  apiKey: string,
  sessionId: string,
  name: string,
  args: Record<string, unknown>,
  approved = false,
): Promise<{ ok: boolean; result: string; tool: string; error?: string; description?: string; preview?: string }> {
  return agentFetch(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}/tools`, {
    method: "POST",
    body: JSON.stringify({ name, arguments: args, approved }),
  });
}

export function approveAgentTool(
  baseUrl: string,
  apiKey: string,
  sessionId: string,
  approvalId: string,
  approved: boolean,
): Promise<{ approval_id: string; approved: boolean }> {
  return agentFetch(baseUrl, apiKey, `/v1/agent/sessions/${sessionId}/approvals/${approvalId}`, {
    method: "POST",
    body: JSON.stringify({ approved }),
  });
}

export async function fetchAgentSkills(baseUrl: string, apiKey: string): Promise<AgentSkill[]> {
  const body = await agentFetch<{ object: string; data: AgentSkill[] }>(baseUrl, apiKey, "/v1/agent/skills");
  return Array.isArray(body?.data) ? body.data : [];
}

export function importAgentSkill(
  baseUrl: string,
  apiKey: string,
  filename: string,
  content: string,
): Promise<AgentSkill> {
  return agentFetch<AgentSkill>(baseUrl, apiKey, "/v1/agent/skills", {
    method: "POST",
    body: JSON.stringify({ filename, content }),
  });
}

export function deleteAgentSkill(baseUrl: string, apiKey: string, alias: string): Promise<{ removed: boolean; alias: string }> {
  return agentFetch(baseUrl, apiKey, `/v1/agent/skills/${encodeURIComponent(alias)}`, { method: "DELETE" });
}

export function setAgentSkillEnabled(
  baseUrl: string,
  apiKey: string,
  alias: string,
  enabled: boolean,
): Promise<{ alias: string; enabled: boolean }> {
  return agentFetch(baseUrl, apiKey, `/v1/agent/skills/${encodeURIComponent(alias)}/enabled`, {
    method: "POST",
    body: JSON.stringify({ enabled }),
  });
}

export function fetchAgentPreferences(baseUrl: string, apiKey: string): Promise<AgentPreferences> {
  return agentFetch<AgentPreferences>(baseUrl, apiKey, "/v1/agent/preferences");
}

export function updateAgentPreferences(
  baseUrl: string,
  apiKey: string,
  prefs: Partial<AgentPreferences>,
): Promise<AgentPreferences> {
  return agentFetch<AgentPreferences>(baseUrl, apiKey, "/v1/agent/preferences", {
    method: "PUT",
    body: JSON.stringify(prefs),
  });
}

export function fetchAgentScreenStatus(baseUrl: string, apiKey: string): Promise<AgentScreenStatus> {
  return agentFetch<AgentScreenStatus>(baseUrl, apiKey, "/v1/agent/screen/status");
}

// --- Screen control — /v1/agent/screen/* (UIA on the gateway host).
// These share the gateway's screen session with the agent loop, so a
// window authorized here becomes the target of computer_inspect too.
// Auth is the same Bearer key as every other gateway call. In
// read-only mode the gateway refuses everything except disarm (403).

export function fetchAgentScreenWindows(baseUrl: string, apiKey: string): Promise<AgentScreenWindows> {
  return agentFetch<AgentScreenWindows>(baseUrl, apiKey, "/v1/agent/screen/list_windows", {
    method: "POST",
    body: JSON.stringify({}),
  });
}

export function authorizeAgentScreenWindow(
  baseUrl: string,
  apiKey: string,
  window: DesktopWindow,
): Promise<AgentScreenAuthorize> {
  return agentFetch<AgentScreenAuthorize>(baseUrl, apiKey, "/v1/agent/screen/authorize", {
    method: "POST",
    body: JSON.stringify(window),
  });
}

export function fetchAgentScreenPreview(baseUrl: string, apiKey: string): Promise<AgentScreenPreview> {
  return agentFetch<AgentScreenPreview>(baseUrl, apiKey, "/v1/agent/screen/preview", {
    method: "POST",
    body: JSON.stringify({}),
  });
}

export function disarmAgentScreen(baseUrl: string, apiKey: string): Promise<{ disarmed: boolean }> {
  return agentFetch<{ disarmed: boolean }>(baseUrl, apiKey, "/v1/agent/screen/disarm", {
    method: "POST",
    body: JSON.stringify({}),
  });
}

export function autoAuthorizeAgentScreen(
  baseUrl: string,
  apiKey: string,
  preferredHint?: string,
): Promise<AgentScreenAuthorize> {
  return agentFetch<AgentScreenAuthorize>(baseUrl, apiKey, "/v1/agent/screen/auto_authorize", {
    method: "POST",
    body: JSON.stringify({ preferred_hint: preferredHint || undefined }),
  });
}

export interface AgentTurnCallbacks {
  onEvent: (ev: AgentTurnEvent) => void;
  onError: (message: string, status?: number) => void;
}

/**
 * POST /v1/agent/sessions/{id}/chat with stream:true; parses SSE `data:`
 * lines and forwards each event object. The stream stays open while the
 * server waits for an approval decision — the caller answers via
 * approveAgentTool on a separate request while this stream is in flight.
 * The final event is `done` ({ok, summary, status}), then `data: [DONE]`.
 */
export async function streamAgentTurn(
  baseUrl: string,
  apiKey: string,
  sessionId: string,
  opts: {
    prompt: string;
    images?: string[];
    replyLanguage?: string;
    turbo?: boolean;
    stream?: boolean;
  },
  cb: AgentTurnCallbacks,
): Promise<void> {
  let res: Response;
  try {
    res = await fetch(`${baseUrl}/v1/agent/sessions/${sessionId}/chat`, {
      method: "POST",
      headers: authHeaders(apiKey),
      body: JSON.stringify({
        prompt: opts.prompt,
        images: opts.images ?? [],
        reply_language: opts.replyLanguage,
        turbo: opts.turbo,
        stream: opts.stream ?? true,
      }),
    });
  } catch (e) {
    cb.onError(`Cannot reach gateway at ${baseUrl}: ${String(e)}`);
    return;
  }
  if (!res.ok) {
    const err = new Error(`POST /v1/agent/sessions/${sessionId}/chat -> ${res.status}: ${await readError(res)}`) as Error & { status?: number };
    err.status = res.status;
    cb.onError(err.message, res.status);
    return;
  }
  if (!res.body) {
    cb.onError("Gateway returned no stream body.");
    return;
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const parts = buf.split("\n\n");
      buf = parts.pop() ?? "";
      for (const part of parts) {
        for (const line of part.split("\n")) {
          const trimmed = line.trim();
          if (!trimmed.startsWith("data:")) continue;
          const payload = trimmed.slice(5).trim();
          if (payload === "[DONE]") return;
          try {
            cb.onEvent(JSON.parse(payload) as AgentTurnEvent);
          } catch {
            // Non-JSON SSE line — ignore.
          }
        }
      }
    }
  } catch (e) {
    cb.onError(`Stream interrupted: ${String(e)}`);
  }
}
