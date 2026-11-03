//! DOM muse.ai — port `Dom.cs` (.NET, dari Python `drivers/browser/dom.py`).
//!
//! Semua pengetahuan tentang struktur halaman tinggal di sini; bila muse.ai
//! mengubah UI, file ini yang (normalnya) perlu diperbarui.
//!
//! Skrip JS dibawa VERBATIM dari `Dom.cs` (interpolasi C# `{{Q(...)}}`
//! sudah diganti nilai literalnya). `Q()` = JSON-quoted string, sama
//! seperti `Dom.Q` (.NET: `JsonSerializer.Serialize(value)`).

use std::collections::HashMap;

/// JSON-quoted string — sama seperti `Dom.Q` (.NET).
pub fn q(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// Selektor input chat.
pub const INPUT: &str = "textarea";
/// Bubble balasan asisten.
pub const AGENT_BUBBLE: &str = r#"div[class*="hatch-agent-bubble-bg"]"#;
/// Lampiran media di chat.
pub const ATTACHMENT: &str = r#"[data-testid^="hatch-chat-attachment-presentation-"]"#;
/// Tombol stop.
pub const STOP_BUTTON: &str = r#"button[aria-label*="Stop" i]"#;
/// Kartu approval inline.
pub const APPROVAL_CARD: &str = r#"[data-testid="hatch-inline-approval-card"]"#;
/// Input file (upload lampiran).
pub const FILE_INPUT: &str = r#"input[type="file"]"#;

/// Petunjuk halaman login (`.NET: LoginHints`).
pub const LOGIN_HINTS: &[&str] = &[
    "log in",
    "sign in",
    "create an account",
    "use another account",
];

/// Petunjuk kuota habis di teks balasan (`.NET: QuotaHints`).
pub const QUOTA_HINTS: &[&str] = &[
    "out of credits",
    "usage limit",
    "limit reached",
    "token limit",
];

/// Petunjuk halaman macet (`.NET: StallHints`).
pub const STALL_HINTS: &[&str] = &["Still sending", "Connecting..."];

/// State halaman: siap / ada input / path / 800 karakter pertama body.
/// Port `Dom.PageState` — `Q(Input)` sudah inline sebagai `"textarea"`.
pub const PAGE_STATE: &str = r##"(() => {
  const body = document.body ? document.body.innerText : '';
  const hydration = document.querySelector('[data-hatch-shell-hydration-state]');
  return {
    ready: document.readyState === 'complete' && !!document.querySelector("textarea")
           && (!hydration || hydration.getAttribute('data-hatch-shell-hydration-state') === 'hydrated')
           && !body.includes('Connecting...'),
    hasInput: !!document.querySelector("textarea"),
    path: location.pathname,
    head: body.slice(0, 800),
  };
})()"##;

/// State chat lengkap — port `Dom.ChatState` (= `ReadChatState(null)`).
/// Token `__REQUEST_ID__` diganti string JSON-quoted oleh [`read_chat_state`].
/// Untuk chat biasa, token diisi `""`.
const CHAT_STATE_TEMPLATE: &str = r##"(() => {
  const visible = e => e.getClientRects().length > 0
    && !e.closest('[hidden],[aria-hidden="true"],[inert]')
    && getComputedStyle(e).visibility !== 'hidden';
  // A rendered <pre><code> no longer contains Markdown fences.
  // Reconstruct them before returning tool JSON to the local executor.
  const replyText = node => {
    if (node.nodeType === Node.TEXT_NODE) return node.nodeValue || '';
    if (node.nodeType !== Node.ELEMENT_NODE) return '';
    const tag = node.tagName;
    if (['BUTTON', 'SVG', 'SCRIPT', 'STYLE'].includes(tag) || node.getAttribute('aria-hidden') === 'true') return '';
    if (tag === 'PRE') {
      const code = node.querySelector('code') || node;
      const language = (code.className || '').match(/(?:language|lang)-([\w-]+)/)?.[1]
        || code.getAttribute('data-language') || node.getAttribute('data-language') || '';
      return '\n```' + language + '\n' + (code.textContent || '') + '\n```\n';
    }
    if (tag === 'BR') return '\n';
    const body = [...node.childNodes].map(replyText).join('');
    return ['DIV', 'P', 'LI', 'UL', 'OL', 'SECTION', 'H1', 'H2', 'H3'].includes(tag) ? body + '\n' : body;
  };
  const scrollers = [...document.querySelectorAll('*')].filter(e => {
    const s = getComputedStyle(e);
    return (s.overflowY === 'auto' || s.overflowY === 'scroll') && e.scrollHeight > e.clientHeight + 50;
  });
  scrollers.forEach(e => { e.scrollTop = e.scrollHeight; });
  const bubbles = [...document.querySelectorAll("div[class*=\"hatch-agent-bubble-bg\"]")].filter(visible);
  const last = bubbles.length ? replyText(bubbles[bubbles.length - 1]).trim() : '';
  // The site can render a branch with the same bubble count and the
  // same JSON as the previous branch. Establish provenance after the
  // new user message instead of mistaking equal text for an old reply.
  const ids = window.__museReplyNodeIds ||= { nodes: new WeakMap(), next: 1 };
  const nodeId = e => { if (!e) return ''; if (!ids.nodes.has(e)) ids.nodes.set(e, String(ids.next++)); return ids.nodes.get(e); };
  const groups = [...document.querySelectorAll('[class~="group/msg"]')].filter(visible);
  const users = groups.filter(e => e.classList.contains('ms-auto') && e.classList.contains('items-end') && !e.querySelector("div[class*=\"hatch-agent-bubble-bg\"]"));
  const user = users.length ? users[users.length - 1] : null;
  const agent = bubbles.length ? bubbles[bubbles.length - 1] : null;
  const userText = user ? replyText(user).trim() : '';
  let fingerprint = 2166136261;
  for (let i=0;i<userText.length;i++) fingerprint = Math.imul(fingerprint ^ userText.charCodeAt(i), 16777619);
  const userKey = user ? userText.length + ':' + (fingerprint >>> 0) : '';
  const replyAfterUser = !!(user && agent && (user.compareDocumentPosition(agent) & Node.DOCUMENT_POSITION_FOLLOWING));
  const requestId = __REQUEST_ID__;
  const owner = requestId ? users.find(e => (e.textContent || '').includes('MUSE_LOCAL_REQUEST_ID: ' + requestId)) : null;
  const follows = (a,b) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
  // Never bind a historical assistant node to a new local submission,
  // even when hydration/reordering changes its text or bubble count.
  const localReplies = owner ? bubbles.filter(b => follows(owner,b)
    && !users.some(u => u !== owner && follows(owner,u) && follows(u,b))) : [];
  const localReply = localReplies.length ? localReplies[localReplies.length-1] : null;
  const localReplyText = localReply ? replyText(localReply).trim() : '';
  const atts = [...document.querySelectorAll("[data-testid^=\"hatch-chat-attachment-presentation-\"]")].map(a => {
    const v = a.querySelector('video'), i = a.querySelector('img');
    const m = v || i;
    return {
      tid: a.getAttribute('data-testid') || '',
      kind: v ? 'video' : (i ? 'image' : 'unknown'),
      src: m ? (m.currentSrc || m.src || '') : '',
      w: m ? (m.videoWidth || m.naturalWidth || 0) : 0,
      h: m ? (m.videoHeight || m.naturalHeight || 0) : 0,
    };
  });
  const tail = document.body ? document.body.innerText.slice(-600) : '';
  const rateLimited = [...document.querySelectorAll('[role="alert"]')].some(e =>
    e.getClientRects().length > 0 && /too many requests|rate limit|sending (?:messages|requests) too (?:fast|quickly)/i.test(e.textContent || ''));
  const buttons = [...document.querySelectorAll('button,[role="button"]')].filter(visible);
  const generating = buttons.some(b => !b.disabled && /^(stop(?: generating| generation| response)?|dừng(?: tạo| phản hồi)?|停止(?:生成)?)(?:[.!…\s]*)$/i.test((b.getAttribute('aria-label') || b.innerText || '').trim()));
  const error = [...document.querySelectorAll('[role="alert"]')].filter(visible)
    .map(e => e.innerText || '').find(t => /something went wrong|failed to (?:send|generate)|connection (?:lost|failed)|network error|đã xảy ra lỗi/i.test(t)) || '';
  return { agentCount: bubbles.length, lastText: last, replyKey: nodeId(agent), replyKeys: bubbles.map(nodeId), userKey, replyAfterUser, rateLimited, error,
            localRequestId: requestId, localReplyBound: !!localReply, localReplyText,
            generating,
            approval: [...document.querySelectorAll("[data-testid=\"hatch-inline-approval-card\"]")].some(visible),
            attachments: atts, tail };
})()"##;

/// Port `Dom.ChatState` — skrip state chat untuk chat biasa.
pub fn chat_state() -> String {
    read_chat_state(None)
}

/// Port `Dom.ReadChatState(localRequestId)` — skrip state chat untuk jalur
/// local-tools (diikat ke `MUSE_LOCAL_REQUEST_ID: <id>`).
pub fn read_chat_state(local_request_id: Option<&str>) -> String {
    CHAT_STATE_TEMPLATE.replace("__REQUEST_ID__", &q(local_request_id.unwrap_or("")))
}

/// Port `Dom.DenyApprovals` — klik tombol deny di semua kartu approval.
pub const DENY_APPROVALS: &str = r##"(() => {
  const denied = [];
  for (const card of document.querySelectorAll("[data-testid=\"hatch-inline-approval-card\"]")) {
    const btn = [...card.querySelectorAll('button,[role=button]')]
      .find(b => /^(deny|decline|reject|don.t allow)$/i.test((b.innerText || '').trim()));
    if (btn && !btn.disabled) {
      btn.click();
      denied.push(card.getAttribute('aria-label') || 'approval request');
    }
  }
  return denied;
})()"##;

/// Port `Dom.LastBubbleMedia` — link media di bubble terakhir.
pub const LAST_BUBBLE_MEDIA: &str = r##"(() => {
  const bubbles = [...document.querySelectorAll("div[class*=\"hatch-agent-bubble-bg\"]")];
  const b = bubbles.length ? bubbles[bubbles.length - 1] : null;
  if (!b) return { links: [], html: '' };
  const urls = [];
  const push = (u) => { if (u && !urls.includes(u)) urls.push(u); };
  b.querySelectorAll('video, source').forEach(m => push(m.currentSrc || m.src || ''));
  b.querySelectorAll('a[href]').forEach(a => push(a.href || ''));
  b.querySelectorAll('img').forEach(m => push(m.currentSrc || m.src || ''));
  const text = b.innerText || '';
  const re = /https?:\/\/[^\s)<>"']+/g;
  let mm; while ((mm = re.exec(text))) push(mm[0]);
  return { links: urls, html: (b.outerHTML || '').slice(0, 2000) };
})()"##;

/// Port `Dom.FillInput(text)` — isi textarea via setter React.
pub fn fill_input(text: &str) -> String {
    r##"(() => {
  const ta = document.querySelector("textarea");
  if (!ta) return { ok: false, err: 'no-input' };
  ta.focus();
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  setter.call(ta, __TEXT__);
  ta.dispatchEvent(new Event('input', { bubbles: true }));
  return { ok: true, len: ta.value.length };
})()"##
        .replace("__TEXT__", &q(text))
}

/// Port `Dom.ClickSend` — klik tombol kirim (mengembalikan
/// `'clicked'` / `'disabled'` / `'missing'`).
pub const CLICK_SEND: &str = r##"(() => {
  const btn = [...document.querySelectorAll('button,[role=button]')]
    .filter(b => b.offsetParent !== null)
    .find(b => /send|发送/i.test((b.getAttribute('aria-label') || '') + ' ' + (b.getAttribute('data-testid') || '')));
  if (!btn) return 'missing';
  if (btn.disabled) return 'disabled';
  btn.click();
  return 'clicked';
})()"##;

/// Port `Dom.InputEmpty` — `true` bila textarea kosong.
pub const INPUT_EMPTY: &str = r##"!(document.querySelector("textarea") || {value: ''}).value"##;

/// Port `Dom.AttachFile(b64, mime, filename)` — lampirkan file via
/// `input[type=file]` + `DataTransfer`.
pub fn attach_file(b64: &str, mime: &str, filename: &str) -> String {
    r##"(() => {
  try {
    const input = document.querySelector("input[type=\"file\"]");
    if (!input) return { ok: false, err: 'no-file-input' };
    const bin = atob(__B64__);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], __FILENAME__, { type: __MIME__ }));
    input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    return { ok: true };
  } catch (e) { return { ok: false, err: String(e) }; }
})()"##
        .replace("__B64__", &q(b64))
        .replace("__FILENAME__", &q(filename))
        .replace("__MIME__", &q(mime))
}

/// Port `Dom.FetchAsBase64(url)` — fetch URL di halaman, kembalikan
/// `{ok, mime, b64}`. Async IIFE: panggil dengan `await_promise=true`.
pub fn fetch_as_base64(url: &str) -> String {
    r##"(async () => {
  try {
    const r = await fetch(__URL__);
    const b = await r.blob();
    const buf = new Uint8Array(await b.arrayBuffer());
    let s = '';
    for (let i = 0; i < buf.length; i += 0x8000) s += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
    return { ok: true, mime: b.type || '', b64: btoa(s) };
  } catch (e) { return { ok: false, err: String(e) }; }
})()"##
        .replace("__URL__", &q(url))
}

/// Cek apakah teks terakhir user tampil di halaman (dipakai
/// `LiveContinuation` .NET). Port ekspresi inline di `BrowserDriver.cs`.
pub fn body_includes(text: &str) -> String {
    format!(
        "!!(document.body && document.body.innerText.includes({}))",
        q(text)
    )
}

// --- kuota (menu Settings) ---

/// Label menu Settings (EN / zh-CN / zh-TW) — `.NET: SettingsLabels`.
pub const SETTINGS_LABELS: &[&str] = &["Settings", "設定", "设置"];
/// Dock "more" — `.NET: SettingsDock`.
pub const SETTINGS_DOCK: &str = r#"[data-testid="hatch-dock-more"]"#;

/// Port `Dom.QuotaSettingsEntry` — posisi tengah tombol pembuka Settings.
pub const QUOTA_SETTINGS_ENTRY: &str = r##"(() => {
  const visible = e => e.getClientRects().length > 0 && getComputedStyle(e).visibility !== 'hidden';
  const labels = ["Settings","設定","设置"];
  const b = [...document.querySelectorAll("[data-testid=\"hatch-dock-more\"]")].find(visible)
    || [...document.querySelectorAll('button,[role=button]')].find(e =>
         visible(e) && labels.includes((e.getAttribute('aria-label') || '').trim()));
  if (!b) return null; const r = b.getBoundingClientRect();
  return {x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2)};
})()"##;

/// Port `Dom.QuotaMenuItem` — posisi tengah item menu Settings.
pub const QUOTA_MENU_ITEM: &str = r##"(() => {
  const visible = e => e.getClientRects().length > 0 && getComputedStyle(e).visibility !== 'hidden';
  const labels = ["Settings","設定","设置"];
  const exact = e => labels.includes((e.textContent || '').trim());
  const b = [...document.querySelectorAll('[role=menuitem]')].find(e =>
      visible(e) && (exact(e) || [...e.querySelectorAll('span,div')].some(exact)))
    || [...document.querySelectorAll('[role=menu] button,[role=menu] span,[role=menu] div')]
         .find(e => visible(e) && exact(e));
  if (!b) return null; const r = b.getBoundingClientRect();
  return {x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2)};
})()"##;

/// Port `Dom.QuotaDialogText` — teks dialog yang sedang terbuka.
pub const QUOTA_DIALOG_TEXT: &str = r##"(() => {
  const d = [...document.querySelectorAll('[role=dialog],[aria-modal=true]')]
    .find(e => e.getClientRects().length > 0 && getComputedStyle(e).visibility !== 'hidden');
  return d ? (d.innerText || '') : '';
})()"##;

// --- parsing teks kuota (EN / zh-CN / zh-TW), port `Dom.ParseUsageText` ---

use regex::Regex;
use std::sync::OnceLock;

fn usage_headings() -> &'static [&'static str] {
    &["usage", "penggunaan", "用量", "使用量"]
}

fn language_headings() -> &'static [&'static str] {
    &["language", "bahasa", "语言", "語言"]
}

fn plan_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:.{1,60}?(?:\s+plan|計劃|计划|方案)|paket\s+\S+)$").expect("plan regex")
    })
}

fn reset_en_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)Weekly limit resets? on ([^\n]+)").expect("reset-en regex")
    })
}

fn reset_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)Batas mingguan diatur ulang pada ([^\n]+)").expect("reset-id regex")
    })
}

fn reset_zh_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?:每(?:週|周|星期)|週|周)(?:上限|限額|限额|額度|额度)(?:會|会|將|将)?(?:於|于|在)\s*([^\n]+?)\s*(?:重設|重置)",
        )
        .expect("reset-zh regex")
    })
}

fn extra_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:(?:Additional|Extra)\s+tokens|Token tambahan|(?:額外|额外)(?:的)?(?:符元|符記|代幣|代币|令牌|憑證|凭证|tokens))",
        )
        .expect("extra regex")
    })
}

fn pct_used_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*%\s*(?:used\b|digunakan\b|已(?:使)?用)").expect("pct-used regex")
    })
}

fn used_pct_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"已(?:使)?用\s*(\d+(?:\.\d+)?)\s*%").expect("used-pct regex"))
}

/// Hasil parsing dialog Usage — port return `Dom.ParseUsageText`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageInfo {
    pub plan: Option<String>,
    pub weekly_used_pct: Option<f64>,
    pub resets_at: Option<String>,
    pub extra_left: Option<String>,
    pub found: bool,
}

impl UsageInfo {
    pub fn empty() -> Self {
        Self {
            plan: None,
            weekly_used_pct: None,
            resets_at: None,
            extra_left: None,
            found: false,
        }
    }

    /// Bentuk map seperti .NET (`Dictionary<string, object?>`) — dipakai
    /// `quota()` driver agar JSON-nya sama dengan .NET.
    pub fn to_map(&self) -> HashMap<String, serde_json::Value> {
        let mut m = HashMap::new();
        m.insert(
            "plan".to_string(),
            self.plan
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        m.insert(
            "weekly_used_pct".to_string(),
            self.weekly_used_pct
                .map(|v| serde_json::json!(v))
                .unwrap_or(serde_json::Value::Null),
        );
        m.insert(
            "resets_at".to_string(),
            self.resets_at
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        m.insert(
            "extra_left".to_string(),
            self.extra_left
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        m.insert("found".to_string(), serde_json::json!(self.found));
        m
    }
}

use serde::Serialize;

fn parse_used_pct(line: &str) -> Option<f64> {
    let caps = pct_used_re()
        .captures(line)
        .or_else(|| used_pct_re().captures(line))?;
    let v: f64 = caps.get(1)?.as_str().parse().ok()?;
    if (0.0..=100.0).contains(&v) {
        Some(v)
    } else {
        None
    }
}

/// Port `Dom.ParseUsageText(text)` 1:1 (termasuk normalisasi FormKC yang
/// diwakili `.chars()` Rust — Rust `char` sudah code point, dan `.NET
/// Normalize(FormKC)` untuk teks dialog ini tidak mengubah hasil regex).
pub fn parse_usage_text(text: Option<&str>) -> UsageInfo {
    let text = text.unwrap_or("");
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(start) = lines
        .iter()
        .position(|l| usage_headings().contains(&l.to_lowercase().as_str()))
    {
        lines = lines.split_off(start + 1);
    }
    if let Some(end) = lines
        .iter()
        .position(|l| language_headings().contains(&l.to_lowercase().as_str()))
    {
        lines.truncate(end);
    }
    let joined = lines.join("\n");

    let plan = lines.iter().find(|l| plan_re().is_match(l)).cloned();
    let mut resets = None;
    if let Some(c) = reset_en_re()
        .captures(&joined)
        .or_else(|| reset_id_re().captures(&joined))
        .or_else(|| reset_zh_re().captures(&joined))
    {
        resets = c.get(1).map(|m| m.as_str().trim().to_string());
    }
    let mut extra_at = lines.iter().position(|l| extra_re().is_match(l));
    if extra_at.is_none() {
        extra_at = Some(lines.len());
    }
    let extra_at = extra_at.unwrap_or(lines.len());
    let mut weekly = None;
    for ln in lines.iter().take(extra_at) {
        if let Some(v) = parse_used_pct(ln) {
            weekly = Some(v);
            break;
        }
    }
    let mut extra_left = None;
    let paren_re = Regex::new(r"\(([^)]+)\)").expect("paren regex");
    for ln in lines.iter().skip(extra_at) {
        if parse_used_pct(ln).is_none() {
            continue;
        }
        if let Some(c) = paren_re.captures(ln) {
            extra_left = c.get(1).map(|m| m.as_str().trim().to_string());
        }
        break;
    }
    UsageInfo {
        plan,
        weekly_used_pct: weekly,
        resets_at: resets,
        extra_left,
        found: weekly.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_quotes_like_dotnet() {
        assert_eq!(q("a\"b\nc"), "\"a\\\"b\\nc\"");
        assert_eq!(q(""), "\"\"");
    }

    #[test]
    fn chat_state_template_has_no_template_tokens_left() {
        let js = chat_state();
        assert!(!js.contains("__REQUEST_ID__"));
        assert!(js.contains("hatch-agent-bubble-bg"));
        assert!(js.contains("MUSE_LOCAL_REQUEST_ID"));
        let local = read_chat_state(Some("abc123"));
        assert!(local.contains("\"abc123\""));
    }

    #[test]
    fn fill_input_embeds_text() {
        let js = fill_input("halo \"dunia\"");
        assert!(js.contains("\"halo \\\"dunia\\\"\""));
        assert!(!js.contains("__TEXT__"));
    }

    #[test]
    fn parse_usage_text_english() {
        let text = "Usage\nFree plan\nWeekly 42.5% used\nWeekly limit resets on Monday\nLanguage";
        let info = parse_usage_text(Some(text));
        assert_eq!(info.weekly_used_pct, Some(42.5));
        assert_eq!(info.resets_at.as_deref(), Some("Monday"));
        assert!(info.found);
        assert_eq!(info.plan.as_deref(), Some("Free plan"));
    }

    #[test]
    fn parse_usage_text_indonesian() {
        // Dialog "Penggunaan" di muse.ai berbahasa Indonesia (3 Nov 2026).
        let text = "Penggunaan\nPaket gratis\nBatas mingguan diatur ulang pada 9 Nov\n48% digunakan\nToken tambahan\nTidak pernah kedaluwarsa\n0% digunakan (sisa 31 M token)\nBahasa";
        let info = parse_usage_text(Some(text));
        assert_eq!(info.weekly_used_pct, Some(48.0));
        assert_eq!(info.resets_at.as_deref(), Some("9 Nov"));
        assert!(info.found);
        assert_eq!(info.plan.as_deref(), Some("Paket gratis"));
        assert_eq!(info.extra_left.as_deref(), Some("sisa 31 M token"));
    }

    #[test]
    fn parse_usage_text_indonesian_ignores_surrounding_text() {
        // Heading "Penggunaan" / "Bahasa" must fence the parsed region:
        // junk plan/percent lines outside the fence must not leak in.
        let text = "Random header\nFree plan\nWeekly 99% used\nPenggunaan\nPaket gratis\nBatas mingguan diatur ulang pada 9 Nov\n48% digunakan\nToken tambahan\nTidak pernah kedaluwarsa\n0% digunakan (sisa 31 M token)\nBahasa\nFree plan\nWeekly 99% used";
        let info = parse_usage_text(Some(text));
        assert_eq!(info.plan.as_deref(), Some("Paket gratis"));
        assert_eq!(info.weekly_used_pct, Some(48.0));
        assert_eq!(info.resets_at.as_deref(), Some("9 Nov"));
        assert_eq!(info.extra_left.as_deref(), Some("sisa 31 M token"));
        assert!(info.found);
    }

    #[test]
    fn parse_usage_text_empty() {
        let info = parse_usage_text(Some(""));
        assert!(!info.found);
        assert_eq!(info.weekly_used_pct, None);
        let info = parse_usage_text(None);
        assert!(!info.found);
    }

    #[test]
    fn parse_usage_text_extra_tokens() {
        let text = "Usage\nPro plan\nWeekly 90% used\nAdditional tokens\nExtra 10% used (5,000 left)";
        let info = parse_usage_text(Some(text));
        assert_eq!(info.weekly_used_pct, Some(90.0));
        assert_eq!(info.extra_left.as_deref(), Some("5,000 left"));
        assert!(info.found);
    }
}
