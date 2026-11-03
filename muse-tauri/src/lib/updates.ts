// Update check via the GitHub Releases API:
// repo anggaset1705/muse-cli, Release feed (Tauri is a packaged
// build), GET https://api.github.com/repos/<repo>/releases/latest,
// 6-hour interval, 8s timeout, ETag-aware cache in localStorage,
// dismiss-per-version. NEVER downloads or runs anything — the banner
// only ever opens the trusted GitHub release page in the browser.
// A trusted link is https://github.com/anggaset1705/muse-cli/
// releases/tag/vX.Y.Z (same rule as DesktopUpdates.IsTrustedLink).

export interface AvailableUpdate {
  version: string;
  url: string;
  name?: string;
}

export type UpdateCheckStatus = "available" | "current" | "no-release" | "unavailable" | "rate-limited";

export interface UpdateCheckResult {
  status: UpdateCheckStatus;
  message: string;
  update?: AvailableUpdate;
}

const REPO = "anggaset1705/muse-cli";
const RELEASE_ENDPOINT = `https://api.github.com/repos/${REPO}/releases/latest`;
const CURRENT_VERSION = "0.1.0";
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000; // 6 jam, seperti DesktopUpdates
const CACHE_KEY = "muse.update-check.v1";

interface Cache {
  checkedAt: number;
  retryNotBefore: number;
  etag?: string;
  dismissedVersion?: string;
  candidate?: AvailableUpdate;
  status: UpdateCheckStatus;
}

function loadCache(): Cache {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (raw) return JSON.parse(raw) as Cache;
  } catch {
    /* ignore */
  }
  return { checkedAt: 0, retryNotBefore: 0, status: "no-release" };
}

function saveCache(c: Cache) {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(c));
  } catch {
    /* ignore */
  }
}

function parseVersion(v: string): [number, number, number] | null {
  const norm = v.replace(/^v/, "").split("+")[0];
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(norm);
  if (!m) return null;
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

export function isNewer(candidate: string, current: string): boolean {
  const c = parseVersion(candidate);
  const cur = parseVersion(current);
  if (!c || !cur) return false;
  for (let i = 0; i < 3; i++) {
    if (c[i] > cur[i]) return true;
    if (c[i] < cur[i]) return false;
  }
  return false;
}

export function isTrustedUpdateLink(update: AvailableUpdate): boolean {
  if (!parseVersion(update.version)) return false;
  let u: URL;
  try {
    u = new URL(update.url);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.hostname !== "github.com" || u.search || u.hash) return false;
  const v = update.version.replace(/^v/, "");
  return u.pathname === `/${REPO}/releases/tag/v${v}` || u.pathname === `/${REPO}/releases/tag/${v}`;
}

export function getDismissedVersion(): string | undefined {
  return loadCache().dismissedVersion;
}

export function dismissUpdate(version: string) {
  const c = loadCache();
  c.dismissedVersion = version;
  saveCache(c);
}

/** Check GitHub releases. `manual=true` bypasses the 6h cache (min 1 min). */
export async function checkForUpdates(manual = false): Promise<UpdateCheckResult> {
  const cache = loadCache();
  const now = Date.now();
  if (cache.retryNotBefore > now) {
    return { status: "rate-limited", message: "rate-limited" };
  }
  if (!manual && cache.checkedAt > 0 && now - cache.checkedAt < CHECK_INTERVAL_MS) {
    if (cache.candidate && isNewer(cache.candidate.version, CURRENT_VERSION)) {
      return { status: "available", message: "cached", update: cache.candidate };
    }
    return { status: cache.status === "available" ? "current" : cache.status, message: "cached" };
  }
  try {
    const res = await fetch(RELEASE_ENDPOINT, {
      headers: {
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        ...(cache.etag ? { "If-None-Match": cache.etag } : {}),
      },
      signal: AbortSignal.timeout(8000),
    });
    if (res.status === 304) {
      cache.checkedAt = now;
      saveCache(cache);
      if (cache.candidate && isNewer(cache.candidate.version, CURRENT_VERSION)) {
        return { status: "available", message: "not-modified", update: cache.candidate };
      }
      return { status: "current", message: "not-modified" };
    }
    if (res.status === 403 || res.status === 429) {
      const retryAfter = res.headers.get("Retry-After");
      const reset = res.headers.get("X-RateLimit-Reset");
      let retry = now + 15 * 60 * 1000;
      if (retryAfter) retry = now + Number(retryAfter) * 1000;
      if (reset) retry = Math.max(retry, Number(reset) * 1000);
      cache.retryNotBefore = retry;
      cache.status = "rate-limited";
      saveCache(cache);
      return { status: "rate-limited", message: "rate-limited" };
    }
    if (res.status === 404) {
      cache.candidate = undefined;
      cache.status = "no-release";
      cache.checkedAt = now;
      saveCache(cache);
      return { status: "no-release", message: "no-release" };
    }
    if (!res.ok) {
      return { status: "unavailable", message: `HTTP ${res.status}` };
    }
    const body = (await res.json()) as { tag_name?: string; html_url?: string; name?: string };
    const etag = res.headers.get("ETag") ?? undefined;
    const version = (body.tag_name ?? "").replace(/^v/, "");
    const update: AvailableUpdate | undefined =
      version && body.html_url
        ? { version, url: body.html_url, name: body.name }
        : undefined;
    cache.etag = etag;
    cache.checkedAt = now;
    cache.candidate = update;
    cache.status = update ? "available" : "no-release";
    saveCache(cache);
    if (update && isNewer(update.version, CURRENT_VERSION)) {
      return { status: "available", message: "ok", update };
    }
    return { status: "current", message: "ok" };
  } catch (e) {
    return { status: "unavailable", message: String(e) };
  }
}

export const APP_VERSION = CURRENT_VERSION;
export const UPDATE_REPO_URL = `https://github.com/${REPO}`;
