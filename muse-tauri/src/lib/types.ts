// Shared types for the Muse Tauri frontend.
//
// HONEST SCOPE — what is real vs. local/stub (see also SCOPE.md):
// - REAL (gateway HTTP): health/ready, models, chat completions (SSE),
//   admin status/accounts/keys/stats, AND the agent loop via
//   /v1/agent/* (sessions, SSE turns with approval, direct tools,
//   changes/revert, skills, preferences, screen status). The brain stays
//   Muse web via the Rust muse-gateway; model id is `muse-chat`.
// - LOCAL (this app only): conversation history, workspace file tree,
//   file read/write, terminal runs, changes + undo, skills (@alias list).
// - STUB (labelled in UI, not connected): Light theme (dark-only for now).
//   Screen CONTROL is REAL: the gateway exposes /v1/agent/screen/*
//   (status, list_windows, authorize, preview, disarm, auto_authorize)
//   backed by UIA on Windows; the Screen tab drives it.

export type Role = "user" | "assistant" | "system";

export interface Attachment {
  name: string;
  /** Text content inlined for text files; null for binary/image (name only). */
  text: string | null;
  kind: "text" | "image" | "other";
  /** data: URL (base64) for images, sent to the agent turn as turn images. */
  dataUrl?: string;
  /** Original file size in bytes (for the 16 MB total image cap, K3). */
  size?: number;
}

export interface ToolPlanAction {
  tool?: string;
  action?: string;
  [key: string]: unknown;
}

/** One action the agent loop is executing, with live status for the UI. */
export type ToolActionStatus =
  | "pending"
  | "running"
  | "done"
  | "error"
  | "awaiting_approval"
  | "denied";

export interface ToolActionState {
  tool: string;
  /** Human-readable description from the executor (if any). */
  description?: string;
  /** Diff/preview text for write/edit actions (if any). */
  preview?: string;
  status: ToolActionStatus;
  /** Tool result text (truncated by the UI when shown). */
  result?: string;
  /** Server approval id while status is awaiting_approval. */
  approvalId?: string;
  /** Workspace-relative path the action targets (if any). */
  path?: string;
}

/** A pending approval the user must answer (Ask mode). */
export interface PendingApproval {
  approvalId: string;
  tool: string;
  description: string;
  preview: string;
  path?: string;
}

// --- /v1/agent/* shapes (see muse-gateway/src/routes.rs) ---

export interface AgentSessionView {
  session_id: string;
  workspace: string;
  model: string;
  reply_language: string;
  turbo: boolean;
  running: boolean;
  history_count: number;
  goal?: unknown;
  updated_at: string;
}

export interface AgentChange {
  path: string;
  kind: string;
  diff: string;
}

export interface AgentSkill {
  alias: string;
  name: string;
  description: string;
  enabled: boolean;
  is_folder: boolean;
  content?: string;
}

export interface AgentPreferences {
  turbo: boolean;
  /** Sent when turboMode is "readonly"; the gateway blocks screen control + tools. Optional: old gateways omit it. */
  read_only?: boolean;
  reply_language: string;
  follow_question_language: boolean;
  ui_language: string;
}

export interface AgentScreenStatus {
  available: boolean;
  computer: string;
  platform: string;
  reason: string;
}

/** One top-level window reported by POST /v1/agent/screen/list_windows
 *  (see muse-gateway/src/routes.rs — the same session the agent loop
 *  uses for computer_inspect, so a window picked here becomes the
 *  agent's screen target too). */
export interface DesktopWindow {
  handle: number;
  process_id: number;
  title: string;
  process_started?: number;
  application_name?: string;
}

export interface AgentScreenWindows {
  windows: DesktopWindow[];
  target: DesktopWindow | null;
}

export interface AgentScreenAuthorize {
  authorized: boolean;
  target: DesktopWindow | null;
}

export interface AgentScreenPreview {
  image_data_url: string | null;
  target: DesktopWindow | null;
}

/** One SSE event from POST /v1/agent/sessions/{id}/chat. */
export interface AgentTurnEvent {
  event: string;
  text?: string;
  tool?: string;
  path?: string;
  approval_id?: string;
  description?: string;
  preview?: string;
  ok?: boolean;
  summary?: string;
  status?: string;
}

export interface ChatMessage {
  id: string;
  role: Role;
  content: string;
  createdAt: number;
  /** "Bahasa Indonesia" | "English" — the reply language requested for this turn. */
  replyLanguage?: string;
  /** Planner JSON {"actions": [...]} decoded from a gateway reply, if any. */
  toolPlan?: ToolPlanAction[];
  /** Actions executed by the agent loop for this turn, with live status. */
  agentActions?: ToolActionState[];
  attachments?: Attachment[];
  /** Gateway / transport error shown instead of a fake reply. */
  error?: string;
  tokensIn?: number;
  tokensOut?: number;
  /** Thinking-card trail (A4): activity lines shown while the turn ran. */
  thoughts?: string[];
  /** Seconds the turn spent thinking (shown as "Thought for Xs"). */
  thinkingSeconds?: number;
  /** Non-error turn outcome: cancelled / declined / incomplete / waiting_input (A6 notice card). */
  turnStatus?: string;
}

export interface Conversation {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  pinned: boolean;
  messages: ChatMessage[];
  /** Model that was active when this conversation was last used (B7: restored on load). */
  model?: string;
}

export type TurboMode = "ask" | "readonly" | "turbo";
export type UiLanguage = "en" | "id";
/** Which Muse driver the sidecar runs: mock echoes (testing only) or the
 * real browser driver (needs a signed-in account + Chromium). */
export type MuseDriver = "mock" | "browser";

export interface Settings {
  uiLanguage: UiLanguage;
  /** When ON, the reply language follows each question (per-message detect). */
  followQuestionLanguage: boolean;
  theme: "dark" | "light";
  turboMode: TurboMode;
  backendBaseUrl: string;
  apiKey: string;
  model: string;
  workspacePath: string;
  shell: string;
  /** Driver the sidecar is started with (forwarded as MUSE2API_DRIVER). */
  driver: MuseDriver;
}

export const DEFAULT_SETTINGS: Settings = {
  uiLanguage: "en",
  followQuestionLanguage: true,
  theme: "dark",
  turboMode: "ask",
  backendBaseUrl: "http://127.0.0.1:18610",
  apiKey: "",
  model: "muse-chat",
  workspacePath: "",
  shell: "",
  driver: "mock",
};

export interface FileEntry {
  name: string;
  rel: string;
  is_dir: boolean;
  size?: number | null;
  children?: FileEntry[] | null;
}

export interface FileChange {
  id: string;
  rel: string;
  kind: "write" | "create";
  createdAt: number;
  /** Previous content (null = file did not exist) so Undo can restore it. */
  previousContent: string | null;
  by: "local-edit" | "terminal" | "agent";
  /** Agent session that made this change (for server-side revert). */
  agentSessionId?: string;
  /** Unified diff text from the server (C11 — shown in the Changes tab). */
  diff?: string;
}

export interface LocalSkill {
  alias: string;
  path: string;
  note: string;
}

export interface BackendStatus {
  running: boolean;
  healthy: boolean;
  base_url: string | null;
  port: number | null;
  pid: number | null;
  launch_mode: string | null;
  /** Reason for the last backend failure (null when none), from Rust. */
  error: string | null;
}

export interface GatewayAccount {
  id: string;
  label?: string;
  status?: string;
  enabled?: boolean;
  /** Masked session cookies from `Account.Public()` (muse-store account.rs):
   * keys present, values masked ("abcd...7890" / "***") — presence of the
   * key is what the connected snapshot checks, never the value. */
  cookies?: Record<string, string>;
  last_used_at?: number;
  created_at?: number;
  [key: string]: unknown;
}

export interface GatewayKey {
  id?: string;
  name?: string;
  label?: string;
  /** First characters of the key, e.g. "m2a-" (WinForms shows "name — prefix…"). */
  prefix?: string;
  revoked?: boolean;
  [key: string]: unknown;
}

export function newConversation(): Conversation {
  const now = Date.now();
  return {
    id: `conv-${now}-${Math.random().toString(36).slice(2, 8)}`,
    title: "New conversation",
    createdAt: now,
    updatedAt: now,
    pinned: false,
    messages: [],
  };
}

export function newMessageId(): string {
  return `msg-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export function formatSize(bytes?: number | null): string {
  if (bytes === undefined || bytes === null) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}k`;
  return `${(bytes / (1024 * 1024)).toFixed(1)}M`;
}

export function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
