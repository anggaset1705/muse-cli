// localStorage persistence for everything that is LOCAL to this app:
// conversations, settings, workspace path, skills, changes.
// Nothing here touches the Rust gateway's own stores.
import type { Conversation, FileChange, LocalSkill, Settings } from "./types";
import { DEFAULT_SETTINGS } from "./types";
import { normalizeWorkspacePath, workspacePathKey } from "./workspacePath";

const KEYS = {
  settings: "muse.settings.v1",
  conversations: "muse.conversations.v1",
  changes: "muse.changes.v1",
  skills: "muse.skills.v1",
};

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return fallback;
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage full / unavailable — the app keeps running in-memory.
  }
}

export function loadSettings(): Settings {
  const saved = readJson<Partial<Settings>>(KEYS.settings, {});
  const merged = { ...DEFAULT_SETTINGS, ...saved };
  // Defect B heal: a workspacePath saved earlier in verbatim form
  // (`\\?\C:\...`) is normalised back on load, so the conversations
  // stored under the plain path show up again immediately.
  if (merged.workspacePath) merged.workspacePath = normalizeWorkspacePath(merged.workspacePath);
  return merged;
}

export function saveSettings(s: Settings): void {
  writeJson(KEYS.settings, s);
}

// --- Conversations, per workspace (B6) ----------------------------------
// WinForms ChatSessionStore keeps one session directory per workspace
// (SHA256 of the canonical workspace path). Tauri mirrors that with one
// localStorage key per workspace path so switching folders switches the
// visible conversation list instead of mixing histories.
// One-time migration: a workspace with no key of its own inherits the
// old global list once, so existing history is not lost on upgrade.
function conversationsKey(workspacePath: string): string {
  const ws = normalizeWorkspacePath(workspacePath || "");
  if (!ws) return KEYS.conversations;
  // Keyed by the CANONICAL path (lowercased on Windows): `C:\FOX`,
  // `c:/FOX`, and `\\?\C:\FOX` are the same workspace, one list.
  return `${KEYS.conversations}::${encodeURIComponent(workspacePathKey(ws))}`;
}

/** Keys a list may historically live under: the canonical key first,
 *  then the raw string as stored, then the verbatim-prefixed form (lists
 *  written while the `\\?\` bug was live). First non-empty list wins and
 *  is migrated to the canonical key. */
function candidateConversationKeys(workspacePath: string): string[] {
  const keys = [conversationsKey(workspacePath)];
  const raw = (workspacePath || "").trim();
  if (raw) {
    const rawKey = `${KEYS.conversations}::${encodeURIComponent(raw)}`;
    if (!keys.includes(rawKey)) keys.push(rawKey);
    const verbatim = raw.startsWith("\\\\?\\") ? raw : `\\\\?\\${raw}`;
    const verbatimKey = `${KEYS.conversations}::${encodeURIComponent(verbatim)}`;
    if (!keys.includes(verbatimKey)) keys.push(verbatimKey);
  }
  return keys;
}

const MIGRATED_KEY = "muse.conversations.v1.migrated-workspaces";

function migratedWorkspaces(): string[] {
  return readJson<string[]>(MIGRATED_KEY, []);
}

export function loadConversations(workspacePath = ""): Conversation[] {
  const key = conversationsKey(workspacePath);
  const list = readJson<Conversation[]>(key, []);
  if (list.length > 0 || !workspacePath.trim() || key === KEYS.conversations) return list;
  // Defect B recovery: the list may live under a legacy raw / verbatim
  // key (written while the `\\?\` bug was live). Adopt it, then migrate
  // it to the canonical key so the next load finds it first try.
  for (const alt of candidateConversationKeys(workspacePath).slice(1)) {
    const found = readJson<Conversation[]>(alt, []);
    if (found.length > 0) {
      writeJson(key, found);
      return found;
    }
  }
  const done = migratedWorkspaces();
  if (done.includes(key)) return list;
  const legacy = readJson<Conversation[]>(KEYS.conversations, []);
  if (legacy.length > 0) {
    writeJson(key, legacy);
    writeJson(MIGRATED_KEY, [...done, key]);
    return legacy;
  }
  return list;
}

export interface SaveConversationsOptions {
  /**
   * The caller explicitly intends to clear the list (user deleted the
   * last conversation). Without this, an empty list NEVER overwrites a
   * stored non-empty list — see the guard in saveConversations.
   */
  allowEmptyOverwrite?: boolean;
}

export function saveConversations(
  list: Conversation[],
  workspacePath = "",
  opts: SaveConversationsOptions = {},
): void {
  const key = conversationsKey(workspacePath);
  if (list.length === 0 && !opts.allowEmptyOverwrite) {
    // Guard (4 Nov 2026 — riwayat "halo apa kabar ?" hilang): the mount
    // save effect in App.tsx persists whatever the in-memory list holds,
    // including [] when the startup load spuriously returned nothing
    // (storage not ready / transient read failure) while a non-empty list
    // was still stored under this key. An empty write must never destroy
    // stored history — only an explicit user delete may clear it.
    // Checked on the RAW stored string, not the parsed value, so even a
    // corrupt-but-present value is preserved instead of being wiped.
    let raw: string | null = null;
    try {
      raw = localStorage.getItem(key);
    } catch {
      raw = null;
    }
    if (raw !== null && raw.trim() !== "" && raw.trim() !== "[]") {
      console.warn(
        `[muse] saveConversations: menolak menimpa daftar tersimpan dengan [] (key ${key}) — kemungkinan load awal gagal; daftar tersimpan dipertahankan.`,
      );
      return;
    }
  }
  writeJson(key, list);
}

export function loadChanges(): FileChange[] {
  return readJson<FileChange[]>(KEYS.changes, []);
}

export function saveChanges(list: FileChange[]): void {
  writeJson(KEYS.changes, list);
}

export function loadSkills(): LocalSkill[] {
  return readJson<LocalSkill[]>(KEYS.skills, []);
}

export function saveSkills(list: LocalSkill[]): void {
  writeJson(KEYS.skills, list);
}
