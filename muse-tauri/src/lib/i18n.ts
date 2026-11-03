// i18n for the Tauri UI — EN/ID strings reused from the WinForms
// UiStrings.cs keys where they exist (NewConversation, Send, Settings,
// Files, Changes, Terminal, Screen, LoginTitle, ...); the rest are new
// keys for this frontend only. UI language and reply language are kept
// SEPARATE on purpose: the WinForms bug was reply language following the
// UI language. Here the reply language follows each question instead
// (see detectReplyLanguage), unless the user turns that off in Settings.

import type { UiLanguage } from "./types";

export type Lang = UiLanguage;

const STRINGS: Record<string, { en: string; id: string }> = {
  appName: { en: "Muse", id: "Muse" },
  appTagline: { en: "Muse Agent", id: "Muse Agent" },
  newConversation: { en: "New conversation", id: "Percakapan baru" },
  searchConversations: { en: "Filter conversations…", id: "Filter percakapan…" },
  all: { en: "All", id: "Semua" },
  pinned: { en: "Pinned", id: "Disematkan" },
  today: { en: "Today", id: "Hari ini" },
  yesterday: { en: "Yesterday", id: "Kemarin" },
  thisWeek: { en: "This week", id: "Minggu ini" },
  older: { en: "Older", id: "Lebih lama" },
  model: { en: "Model", id: "Model" },
  workspace: { en: "Workspace", id: "Workspace" },
  settings: { en: "Settings", id: "Pengaturan" },
  transcript: { en: "Transcript", id: "Transkrip" },
  json: { en: "JSON", id: "JSON" },
  import: { en: "Import", id: "Impor" },
  login: { en: "Connect Muse account", id: "Hubungkan akun Muse" },
  loginTitle: { en: "Connect your Muse account", id: "Hubungkan akun Muse Anda" },
  loginHeading: { en: "Sign in to your Muse account", id: "Masuk ke akun Muse Anda" },
  loginHeadingDesc: {
    en: "Sign in right on the Muse page. The app saves the connection automatically when you finish.",
    id: "Masuk langsung di halaman Muse. Aplikasi menyimpan koneksi otomatis setelah Anda selesai.",
  },
  loginCookieLabel: { en: "Account label", id: "Label akun" },
  loginInApp: { en: "Sign in inside the app", id: "Masuk di dalam app" },
  loginInAppDesc: {
    en: "Opens a Muse sign-in window inside the app (separate profile — your own browser is not touched). Sign in there; the app saves the account automatically.",
    id: "Membuka jendela masuk Muse di dalam app (profil terpisah — browser Anda tidak disentuh). Masuk di sana; app menyimpan akun otomatis.",
  },
  loginWindowOpened: {
    en: "Sign-in window opened — sign in there, then come back here.",
    id: "Jendela masuk dibuka — masuk di sana, lalu kembali ke sini.",
  },
  loginWindowClosed: {
    en: "Sign-in window closed.",
    id: "Jendela masuk ditutup.",
  },
  loginCheckAccounts: {
    en: "Checking saved accounts…",
    id: "Memeriksa akun tersimpan…",
  },
  loginDone: {
    en: "Account saved to the gateway.",
    id: "Akun tersimpan di gateway.",
  },
  loginNoAccount: {
    en: "No new account was saved — the window was closed before signing in.",
    id: "Tidak ada akun baru tersimpan — jendela ditutup sebelum masuk.",
  },
  // Shown in the login dialog when it was opened because the user tried
  // to chat with no Muse account connected (send gate in App.tsx).
  loginNeededToChat: {
    en: "No Muse account is connected yet. Sign in below to start chatting — your message is kept in the chat box, just send it again after signing in.",
    id: "Belum ada akun Muse yang terhubung. Masuk di bawah untuk mulai chat — pesan Anda tetap tersimpan di kotak chat, tinggal kirim lagi setelah masuk.",
  },
  loginAlreadyOpen: {
    en: "The sign-in window is already open.",
    id: "Jendela masuk sudah terbuka.",
  },
  loginNeedsGateway: {
    en: "In-app sign-in needs the Rust gateway (muse-gateway.exe) next to the app.",
    id: "Masuk di dalam app butuh gateway Rust (muse-gateway.exe) di samping app.",
  },
  driverLabel: { en: "Muse driver", id: "Driver Muse" },
  driverMock: {
    en: "Mock — echo replies, no real Muse (testing only)",
    id: "Mock — jawaban gema, bukan Muse asli (hanya untuk tes)",
  },
  driverBrowser: {
    en: "Browser — real Muse (needs a signed-in account + Chromium)",
    id: "Browser — Muse asli (butuh akun yang sudah masuk + Chromium)",
  },
  driverNote: {
    en: "Changing the driver restarts the backend. The browser driver needs Chromium (Playwright).",
    id: "Mengganti driver akan me-restart backend. Driver browser butuh Chromium (Playwright).",
  },
  // Defect B: shown ONLY while the backend is not healthy — as a standing
  // note next to a healthy backend it read as "backend fails to start".
  driverInstallNote: {
    en: "If the backend fails to start, install Chromium first, then try Start backend again.",
    id: "Bila backend gagal start, instal Chromium dulu, lalu coba Start backend lagi.",
  },
  messagePlaceholder: { en: "Message Muse…", id: "Pesan Muse…" },
  send: { en: "Send", id: "Kirim" },
  copy: { en: "Copy", id: "Salin" },
  copied: { en: "Copied", id: "Disalin" },
  messages: { en: "messages", id: "pesan" },
  you: { en: "You", id: "Anda" },
  museAgent: { en: "Muse Agent", id: "Muse Agent" },
  thinking: { en: "Thinking…", id: "Berpikir…" },
  toolPlan: { en: "Tool plan", id: "Rencana tool" },
  toolPlanNote: {
    en: "Plan returned by the gateway. Execution is NOT connected to the agent loop — this is a plan only.",
    id: "Rencana dari gateway. Eksekusi TIDAK tersambung ke loop agen — ini hanya rencana.",
  },
  agentActions: { en: "Agent actions", id: "Aksi agen" },
  agentActionsNote: {
    en: "Executed by the agent loop via /v1/agent/*. Ask mode waits for your approval before writes, edits and commands.",
    id: "Dieksekusi loop agen via /v1/agent/*. Mode Tanya menunggu persetujuan Anda sebelum tulis, edit, dan perintah.",
  },
  agentRunning: { en: "running…", id: "berjalan…" },
  agentDone: { en: "done", id: "selesai" },
  agentAwaitingApproval: { en: "waiting for your approval", id: "menunggu persetujuan Anda" },
  agentDenied: { en: "denied", id: "ditolak" },
  approve: { en: "Approve", id: "Setujui" },
  deny: { en: "Deny", id: "Tolak" },
  agentTurnFailed: {
    en: "Agent turn did not complete.",
    id: "Giliran agen tidak selesai.",
  },
  agentUnavailableFallback: {
    en: "Agent endpoints are not on this gateway (older build) — falling back to plain chat. Planner actions will not execute.",
    id: "Endpoint agen tidak ada di gateway ini (build lama) — kembali ke chat biasa. Aksi planner tidak akan dieksekusi.",
  },
  agentNeedsWorkspace: {
    en: "Pick a workspace folder first — agent turns run inside a workspace.",
    id: "Pilih folder workspace dulu — giliran agen berjalan di dalam workspace.",
  },
  changeByAgent: { en: "agent", id: "agen" },
  files: { en: "Files", id: "File" },
  changes: { en: "Changes", id: "Perubahan" },
  terminal: { en: "Terminal", id: "Terminal" },
  screen: { en: "Screen", id: "Layar" },
  refresh: { en: "Refresh", id: "Segarkan" },
  collapse: { en: "Collapse", id: "Ciutkan" },
  expand: { en: "Expand", id: "Bentangkan" },
  openFile: { en: "Open", id: "Buka" },
  save: { en: "Save", id: "Simpan" },
  undo: { en: "Undo", id: "Urungkan" },
  run: { en: "Run", id: "Jalankan" },
  stop: { en: "Stop", id: "Berhenti" },
  commandPlaceholder: { en: "Type a command to run in the workspace…", id: "Ketik perintah untuk dijalankan di workspace…" },
  noWorkspace: {
    en: "No workspace selected — pick a folder below to browse files, edit, and run terminal commands.",
    id: "Belum ada workspace — pilih folder di bawah untuk menjelajah file, mengedit, dan menjalankan terminal.",
  },
  workspacePath: { en: "Workspace folder path", id: "Path folder workspace" },
  screenUnavailable: {
    en: "Screen control is unavailable in the gateway host. Use the Muse desktop application (Windows) for screen control.",
    id: "Kontrol layar tidak tersedia di host gateway. Pakai aplikasi desktop Muse (Windows) untuk kontrol layar.",
  },
  screenUnreachable: {
    en: "Could not reach the gateway to check screen status.",
    id: "Tidak bisa menghubungi gateway untuk memeriksa status layar.",
  },
  // --- Screen picker (A: window picker + live preview, /v1/agent/screen/*) ---
  screenSearch: { en: "Search windows…", id: "Cari jendela…" },
  screenWindowCount: { en: "{0} windows", id: "{0} jendela" },
  screenNoWindows: { en: "No windows found.", id: "Tidak ada jendela ditemukan." },
  screenSelectHint: {
    en: "Select a window — the agent will see and control it. Double-click to allow.",
    id: "Pilih jendela — agen akan melihat dan mengendalikannya. Klik ganda untuk mengizinkan.",
  },
  screenAllow: { en: "Allow", id: "Izinkan" },
  screenAutoSelect: { en: "Auto-select", id: "Pilih otomatis" },
  screenDisarm: { en: "Disarm", id: "Matikan" },
  screenAuthorized: { en: "Authorized", id: "Terotorisasi" },
  screenLivePreview: { en: "Live preview", id: "Preview live" },
  screenNoTarget: { en: "No window selected yet.", id: "Belum ada jendela terpilih." },
  screenPreviewUnavailable: { en: "Preview unavailable.", id: "Preview tidak tersedia." },
  screenReadonlyNote: {
    en: "Screen control is blocked while approval mode is Read-only.",
    id: "Kontrol layar diblokir saat mode persetujuan Hanya-baca.",
  },
  screenUntitled: { en: "(untitled window)", id: "(jendela tanpa judul)" },
  localBadge: { en: "LOCAL", id: "LOKAL" },
  stubBadge: { en: "STUB", id: "STUB" },
  liveBadge: { en: "LIVE", id: "LIVE" },
  backendStopped: { en: "Backend stopped", id: "Backend berhenti" },
  backendStarting: { en: "Backend starting — waiting for /healthz…", id: "Backend mulai — menunggu /healthz…" },
  backendHealthy: { en: "Backend healthy", id: "Backend sehat" },
  backendUnreachable: {
    en: "Gateway unreachable. Start the backend (sidebar) or check the Base URL in Settings.",
    id: "Gateway tidak terjangkau. Jalankan backend (sidebar) atau periksa Base URL di Pengaturan.",
  },
  startBackend: { en: "Start backend", id: "Jalankan backend" },
  stopBackend: { en: "Stop backend", id: "Hentikan backend" },
  settingsLanguage: { en: "Language / Bahasa", id: "Bahasa / Language" },
  settingsFollowQuestion: { en: "Follow question language", id: "Ikuti bahasa pertanyaan" },
  settingsFollowQuestionDesc: {
    en: "When ON, Muse replies in the language of each question (Indonesian vs English), not the UI language. Default ON.",
    id: "Bila ON, Muse membalas dalam bahasa tiap pertanyaan (Indonesia vs Inggris), bukan bahasa UI. Default ON.",
  },
  settingsTheme: { en: "Theme", id: "Tema" },
  settingsThemeDarkOnly: {
    en: "Dark only for now — Light theme is a stub.",
    id: "Untuk saat ini hanya Gelap — tema Terang masih stub.",
  },
  settingsTurbo: { en: "Approval mode (local tools)", id: "Mode persetujuan (tool lokal)" },
  settingsTurboDesc: {
    en: "Applies to LOCAL file/terminal actions in this app AND to agent turns (Turbo = run without asking; Ask = approve each write/edit/command). Synced to the gateway on save when reachable.",
    id: "Berlaku untuk aksi file/terminal LOKAL di aplikasi ini DAN untuk giliran agen (Turbo = jalan tanpa bertanya; Tanya = setujui tiap tulis/edit/perintah). Disinkronkan ke gateway saat disimpan bila terjangkau.",
  },
  turboAsk: { en: "Ask before each action", id: "Tanya sebelum tiap aksi" },
  turboReadonly: { en: "Read-only (block writes)", id: "Hanya-baca (blokir tulis)" },
  turboTurbo: { en: "Turbo (run without asking)", id: "Turbo (jalan tanpa bertanya)" },
  settingsBackendUrl: { en: "Backend base URL", id: "Base URL backend" },
  settingsApiKey: { en: "Gateway API key", id: "API key gateway" },
  settingsAccounts: { en: "Accounts (via /admin/accounts)", id: "Akun (via /admin/accounts)" },
  settingsKeys: { en: "API keys (via /admin/keys)", id: "API key (via /admin/keys)" },
  settingsSkills: { en: "Skills (local @alias list)", id: "Skills (daftar @alias lokal)" },
  settingsSkillsNote: {
    en: "Server skills (from /v1/agent/skills) are used by the agent loop. The local @alias list below is kept for reference only.",
    id: "Skill server (dari /v1/agent/skills) dipakai loop agen. Daftar @alias lokal di bawah hanya untuk referensi.",
  },
  settingsServerSkills: { en: "Server skills (agent loop)", id: "Skill server (loop agen)" },
  settingsServerSkillsEmpty: {
    en: "No server skills yet.",
    id: "Belum ada skill server.",
  },
  skillAlias: { en: "Alias (e.g. @my-skill)", id: "Alias (mis. @my-skill)" },
  skillPath: { en: "Path or note", id: "Path atau catatan" },
  add: { en: "Add", id: "Tambah" },
  remove: { en: "Remove", id: "Hapus" },
  accountsEmpty: { en: "(no accounts yet)", id: "(belum ada akun)" },
  noKeys: { en: "No keys returned.", id: "Tidak ada key." },
  noMessages: {
    en: "No messages yet. Ask Muse anything — replies follow the language of your question.",
    id: "Belum ada pesan. Tanya apa saja ke Muse — balasan mengikuti bahasa pertanyaan Anda.",
  },
  noConversations: { en: "No conversations yet.", id: "Belum ada percakapan." },
  attachedFiles: { en: "Attached files", id: "File terlampir" },
  attachNote: {
    en: "Text files are sent inline. Images are sent to the agent turn (max 4, 8 MB each).",
    id: "File teks dikirim inline. Gambar dikirim ke giliran agen (maks 4, 8 MB tiap file).",
  },
  confirmLocalAction: {
    en: "Run this local action?",
    id: "Jalankan aksi lokal ini?",
  },
  readonlyBlocked: {
    en: "Blocked: approval mode is Read-only.",
    id: "Diblokir: mode persetujuan Hanya-baca.",
  },
  noChanges: { en: "No local changes yet.", id: "Belum ada perubahan lokal." },
  changeByLocalEdit: { en: "local edit", id: "edit lokal" },
  changeByTerminal: { en: "terminal", id: "terminal" },
  deleteConversation: { en: "Delete conversation", id: "Hapus percakapan" },
  pinConversation: { en: "Pin", id: "Sematkan" },
  unpinConversation: { en: "Unpin", id: "Lepas sematan" },
  exportTranscript: { en: "Download transcript (.md)", id: "Unduh transkrip (.md)" },
  exportJson: { en: "Download JSON", id: "Unduh JSON" },
  importConversation: { en: "Import conversation JSON", id: "Impor JSON percakapan" },
  backendStatus: { en: "Backend", id: "Backend" },
  profileDefault: { en: "default", id: "default" },
  close: { en: "Close", id: "Tutup" },
  cancel: { en: "Cancel", id: "Batal" },
  confirm: { en: "Confirm", id: "Konfirmasi" },
  // --- Thinking card (A4, port of ChatTranscript.ThinkingCard) ---
  thinkingTitle: { en: "Thinking", id: "Berpikir" },
  thoughtFor: { en: "Thought for {0}s", id: "Berpikir selama {0} dtk" },
  thinkingSeconds: { en: "{0}s", id: "{0} dtk" },
  goalRunning: {
    en: "Goal running — building, testing, and fixing…",
    id: "Tujuan berjalan — membangun, menguji, dan memperbaiki…",
  },
  // --- Welcome screen (A17, port of AgentWorkspaceControl welcome) ---
  welcomeEyebrow: { en: "FROM IDEA TO PRODUCT", id: "DARI IDE MENJADI PRODUK" },
  welcomeHeading: { en: "Create something new with Muse.", id: "Ciptakan hal baru bersama Muse." },
  welcomeHintDefault: {
    en: "Choose a project, connect your Muse account, then describe what you want to do.",
    id: "Pilih proyek, hubungkan akun Muse, lalu jelaskan yang ingin Anda lakukan.",
  },
  welcomeHintNoProject: {
    en: "Choose a project folder on your computer. Muse will help you read, edit, and test it with plain requests.",
    id: "Pilih folder proyek di komputer Anda. Muse akan membantu Anda membaca, mengubah, dan mengujinya dengan permintaan biasa.",
  },
  welcomeHintNoConnection: {
    en: "The project is ready. Connect your Muse account to start working.",
    id: "Proyek sudah siap. Hubungkan akun Muse untuk mulai bekerja.",
  },
  welcomeHintReady: {
    en: "Describe what you want to do. Muse will read the project, suggest an approach, and ask before changing anything.",
    id: "Jelaskan yang ingin Anda lakukan. Muse akan membaca proyek, mengusulkan cara kerja, dan bertanya dulu sebelum mengubah apa pun.",
  },
  welcomeChooseProject: { en: "Choose project", id: "Pilih proyek" },
  welcomeConnect: { en: "Connect Muse", id: "Hubungkan Muse" },
  welcomeConnected: { en: "Connected", id: "Terhubung" },
  welcomeNotConnected: { en: "Not connected", id: "Belum terhubung" },
  welcomeProjectSelected: { en: "Project selected", id: "Proyek dipilih" },
  suggestExplore: { en: "Explore the project", id: "Pelajari proyek" },
  suggestExplorePrompt: {
    en: "Read the project and briefly explain its structure, main features, and how they work.",
    id: "Baca proyek ini dan jelaskan secara singkat struktur, fitur utama, dan cara kerjanya.",
  },
  suggestExploreDesc: { en: "Read and understand the source structure", id: "Baca dan pahami struktur kode sumber" },
  suggestFix: { en: "Find and fix bugs", id: "Cari dan perbaiki bug" },
  suggestFixPrompt: {
    en: "Check the project, find the important bugs, explain how to fix them, and show me the changes before applying them.",
    id: "Periksa proyek, temukan bug yang penting, jelaskan cara memperbaikinya, dan tunjukkan perubahannya sebelum diterapkan.",
  },
  suggestFixDesc: { en: "Find the cause, preview the fix", id: "Temukan penyebabnya, lihat dulu perbaikannya" },
  suggestFeature: { en: "Add a feature", id: "Tambah fitur" },
  suggestFeaturePrompt: {
    en: "I want to add a feature to the project: ",
    id: "Saya ingin menambahkan fitur ke proyek ini: ",
  },
  suggestFeatureDesc: { en: "Turn an idea into a feature", id: "Ubah ide menjadi fitur" },
  suggestQuality: { en: "Check quality", id: "Periksa kualitas" },
  suggestQualityPrompt: {
    en: "Check the current changes and run the appropriate tests. Explain the results in plain language.",
    id: "Periksa perubahan saat ini dan jalankan pengujian yang sesuai. Jelaskan hasilnya dengan bahasa yang mudah dipahami.",
  },
  suggestQualityDesc: { en: "Run tests and view the results", id: "Jalankan pengujian dan lihat hasilnya" },
  // --- Stop / abort (A12, J1) ---
  stopTurn: { en: "Stop turn", id: "Hentikan giliran" },
  stopTurnTitle: { en: "Stop this turn (Esc)", id: "Hentikan giliran ini (Esc)" },
  abortTurn: { en: "Stop turn", id: "Hentikan giliran" },
  turnCancelled: { en: "Turn stopped.", id: "Giliran dihentikan." },
  turnDeclined: { en: "The action was declined.", id: "Aksi ditolak." },
  turnIncomplete: { en: "The turn did not finish.", id: "Giliran tidak selesai." },
  stopping: { en: "Stopping…", id: "Menghentikan…" },
  // --- Tool cards (A5) ---
  viewTerminal: { en: "View terminal", id: "Lihat terminal" },
  toolPreview: { en: "Preview", id: "Pratinjau" },
  // --- Editor (C7–C9) ---
  editorUnsaved: { en: " · Unsaved", id: " · Belum disimpan" },
  editorDiscardConfirm: {
    en: "Discard unsaved changes to this file?",
    id: "Buang perubahan yang belum disimpan di file ini?",
  },
  fileChangedOnDisk: {
    en: "The file changed on disk. Reopen the file before saving.",
    id: "File berubah di disk. Buka ulang file sebelum menyimpan.",
  },
  fileReload: { en: "Reload file", id: "Muat ulang file" },
  askMuse: { en: "Ask Muse", id: "Tanya Muse" },
  askMuseFilePrompt: { en: 'Read and explain the file "{0}". ', id: 'Baca dan jelaskan file "{0}". ' },
  editorTooLarge: {
    en: "This file is larger than the 2 MB the local reader supports. Open it in an external editor.",
    id: "File ini lebih besar dari 2 MB yang didukung pembaca lokal. Buka di editor eksternal.",
  },
  fileSaved: { en: "Saved {0}", id: "Tersimpan {0}" },
  // --- Terminal (D2/D4/D5) ---
  runInBackground: { en: "Run in background", id: "Jalankan di background" },
  backgroundFinished: { en: "Background command finished.", id: "Perintah background selesai." },
  sendResultToMuse: { en: "Send result to Muse", id: "Kirim hasil ke Muse" },
  terminalResultPrefix: { en: "Terminal result for", id: "Hasil terminal untuk" },
  // --- Accounts & keys (F8/F9, G1–G6, G8/G9) ---
  showKey: { en: "Show key", id: "Lihat key" },
  hideKey: { en: "Hide key", id: "Sembunyikan key" },
  copyKey: { en: "Copy", id: "Salin" },
  keyCopied: { en: "Copied to clipboard", id: "Disalin ke clipboard" },
  enableAccount: { en: "Enable", id: "Aktifkan" },
  disableAccount: { en: "Disable", id: "Nonaktifkan" },
  deleteAccount: { en: "Delete account", id: "Hapus akun" },
  deleteAccountConfirm: {
    en: "Remove account '{0}' from Muse? This account's sign-in session will be removed.",
    id: "Hapus akun '{0}' dari Muse? Sesi login akun ini akan dihapus.",
  },
  accountDisabledSuffix: { en: " (disabled)", id: " (nonaktif)" },
  // --- Account management port (WinForms PreferencesForm / UiStrings.cs) ---
  // Status line (snapshot, not a live check — see hasSavedLogin in gateway.ts).
  accountConnected: { en: "Muse account connected", id: "Akun Muse terhubung" },
  accountNotConnected: { en: "Muse account not connected", id: "Akun Muse belum terhubung" },
  openLogin: { en: "Open / restore Muse sign-in", id: "Buka / pulihkan login Muse" },
  accountsHint: {
    en: "Multiple accounts can sign in and work at the same time; the gateway shares turns between the accounts that are on.",
    id: "Beberapa akun bisa masuk dan bekerja bersamaan; gateway membagi giliran antar akun yang aktif.",
  },
  addAccount: { en: "Add account", id: "Tambah akun" },
  accountNumber: { en: "Account {0}", id: "Akun {0}" },
  accountsLoadFailed: { en: "Could not load the account list: {0}", id: "Tidak bisa memuat daftar akun: {0}" },
  // Login success naming the saved account (best effort — LoginDialog falls
  // back to the total-count message when the label cannot be matched).
  loginSavedAccount: { en: "Account '{0}' saved to the gateway.", id: "Akun '{0}' tersimpan di gateway." },
  keyRevokedSuffix: { en: " (revoked)", id: " (dicabut)" },
  createKey: { en: "Create key", id: "Buat key" },
  keyCreatedTitle: { en: "New API key", id: "API key baru" },
  keyCreatedBody: {
    en: "New API key (copied to the clipboard):\n\n{0}\n\nPaste it into the app that needs to connect now; the key will not be shown again.",
    id: "API key baru (sudah disalin ke clipboard):\n\n{0}\n\nTempel ke aplikasi yang perlu terhubung sekarang; key tidak akan ditampilkan lagi.",
  },
  revokeKey: { en: "Revoke", id: "Cabut" },
  restoreKey: { en: "Restore", id: "Pulihkan" },
  deleteKey: { en: "Delete key", id: "Hapus key" },
  deleteKeyConfirm: {
    en: "Delete key '{0}' for good? Apps using this key will lose their connection.",
    id: "Hapus key '{0}' selamanya? Aplikasi yang memakai key ini akan kehilangan koneksi.",
  },
  selectAccountFirst: { en: "Select an account first.", id: "Pilih akun dulu." },
  selectKeyFirst: { en: "Select a key first.", id: "Pilih key dulu." },
  quota: { en: "Quota", id: "Kuota" },
  // --- Usage widget (sidebar, below MODEL) ---
  usage: { en: "Usage", id: "Penggunaan" },
  usageRefresh: { en: "Refresh usage", id: "Refresh penggunaan" },
  usageLoading: { en: "Loading…", id: "Memuat…" },
  usageUnavailable: { en: "Not available", id: "Tidak tersedia" },
  usageWeekly: { en: "{0}% used", id: "{0}% digunakan" },
  usageReset: { en: "Resets {0}", id: "Direset {0}" },
  usageExtra: { en: "Extra tokens: {0}", id: "Token tambahan: {0}" },
  usageUpdated: { en: "Updated {0}", id: "Diperbarui {0}" },
  renew: { en: "Renew", id: "Perbarui" },
  renewConfirm: { en: "Renew account '{0}'?", id: "Perbarui akun '{0}'?" },
  adminStatus: { en: "Admin status", id: "Status admin" },
  // --- Skills (H3/H4) ---
  importSkill: { en: "Import skill file", id: "Impor file skill" },
  importSkillNote: {
    en: "Pick a skill file (.md) to import into the gateway's skill store.",
    id: "Pilih file skill (.md) untuk diimpor ke skill store gateway.",
  },
  deleteSkill: { en: "Delete skill", id: "Hapus skill" },
  deleteSkillConfirm: {
    en: "Delete server skill '@{0}' for good?",
    id: "Hapus skill server '@{0}' selamanya?",
  },
  // --- Settings: theme / advanced / updates (I2, I10–I13) ---
  themeDark: { en: "Dark", id: "Gelap" },
  themeLight: { en: "Light", id: "Terang" },
  settingsThemeNote: {
    en: "Light theme uses the WinForms LightPalette. Saved with settings.",
    id: "Tema terang memakai LightPalette WinForms. Tersimpan bersama pengaturan.",
  },
  advanced: { en: "Advanced", id: "Lanjutan" },
  collapseAdvanced: { en: "Hide advanced", id: "Sembunyikan lanjutan" },
  openWorkspaceFolder: { en: "Open workspace folder", id: "Buka folder workspace" },
  openAppDataFolder: { en: "Open app data folder", id: "Buka folder data aplikasi" },
  openDataFolderNote: {
    en: "Workspace folder: the project the agent works in. App data folder: where the gateway keeps accounts, agent sessions and keys.",
    id: "Folder workspace: project yang dikerjakan agent. Folder data aplikasi: tempat gateway menyimpan akun, sesi agent, dan key.",
  },
  diagnosticsLog: { en: "Diagnostics log", id: "Log diagnostik" },
  noSetupErrors: { en: "No setup errors.", id: "Tidak ada error setup." },
  updateSection: { en: "Updates", id: "Pembaruan" },
  updateCurrent: { en: "You are on the latest version.", id: "Anda memakai versi terbaru." },
  updateAvailable: { en: "Update available: {0}", id: "Pembaruan tersedia: {0}" },
  updateCheck: { en: "Check for updates", id: "Periksa pembaruan" },
  updateChecking: { en: "Checking for updates…", id: "Memeriksa pembaruan…" },
  updateUnavailable: { en: "Could not check for updates.", id: "Tidak bisa memeriksa pembaruan." },
  updateRateLimited: { en: "GitHub is rate-limiting update checks. Try again later.", id: "GitHub sedang membatasi pemeriksaan pembaruan. Coba lagi nanti." },
  viewUpdate: { en: "View update", id: "Lihat pembaruan" },
  dismissUpdate: { en: "Later", id: "Nanti" },
  updateNote: {
    en: "Checks GitHub releases (anggaset1705/muse-cli) at most every 6 hours. Never downloads or installs anything automatically.",
    id: "Memeriksa GitHub releases (anggaset1705/muse-cli) maksimal tiap 6 jam. Tidak pernah mengunduh atau memasang apa pun otomatis.",
  },
  // --- Login (F4/F6) ---
  loginOtherAccount: { en: "Other account", id: "Akun lain" },
  loginOtherAccountNote: {
    en: "Clears the saved sign-in session and opens a fresh window — sign in there with a different Muse account.",
    id: "Membersihkan sesi masuk yang tersimpan dan membuka jendela baru — masuk di sana dengan akun Muse yang lain.",
  },
  // --- Login mode banners (Cacat A, 3 Nov 2026) ---
  // The sidebar icon opens the dialog in RESTORE mode (saved session is
  // reused); Settings "Add account" opens it in ADD mode (session
  // cleared). Bree used the restore path with a new label and the old
  // account was renamed instead of a second account being added — the
  // dialog now says which mode it is in, out loud.
  loginModeRestore: {
    en: "Mode: restore the saved account",
    id: "Mode: pulihkan akun yang ada",
  },
  loginModeAdd: {
    en: "Mode: add a new account — the sign-in session is cleared first",
    id: "Mode: tambah akun baru — sesi masuk dibersihkan dulu",
  },
  loginExistingAccounts: {
    en: "Saved account(s): {0}",
    id: "Akun tersimpan: {0}",
  },
  loginLabelMismatch: {
    en: "The label '{0}' does not match any saved account. The main button below will restore the saved account ({1}) and only rename it — it will NOT add a second account. To add another account, use 'Add another account' below.",
    id: "Label '{0}' tidak cocok dengan akun tersimpan. Tombol utama di bawah akan memulihkan akun tersimpan ({1}) dan hanya mengganti namanya — TIDAK menambah akun kedua. Untuk menambah akun, pakai 'Tambah akun lain' di bawah.",
  },
  // Confirm shown BEFORE signing in (normal mode, mismatched label):
  // continuing renames the saved account, it does not add a new one.
  loginRenameConfirm: {
    en: "The label '{0}' does not match any saved account. Continuing will rename the saved account ({1}) to '{0}' — it will NOT add a second account. Continue anyway?",
    id: "Label '{0}' tidak cocok dengan akun tersimpan. Lanjut berarti mengganti nama akun tersimpan ({1}) jadi '{0}' — BUKAN menambah akun baru. Tetap lanjut?",
  },
  loginSecondAccountNote: {
    en: "A real second account is only possible by signing in with a DIFFERENT Muse account — signing in with the same account merges into the saved one (its label is overwritten).",
    id: "Akun kedua sungguhan hanya bisa dengan masuk memakai akun Muse yang BERBEDA — masuk dengan akun yang sama akan bergabung ke akun tersimpan (labelnya ditimpa).",
  },
  loginAddAccountCta: {
    en: "Add another account (clear session)",
    id: "Tambah akun lain (bersihkan sesi)",
  },
  loginRetry: { en: "Retry", id: "Coba lagi" },
  // --- Misc ---
  scrollToBottom: { en: "Scroll to bottom", id: "Gulir ke bawah" },
  pasteImageHint: {
    en: "Take a screenshot, then press Ctrl+V in the chat box to attach an image.",
    id: "Ambil screenshot, lalu tekan Ctrl+V di kotak chat untuk melampirkan gambar.",
  },
  dragDropHint: { en: "Drop images here to attach them.", id: "Jatuhkan gambar di sini untuk melampirkannya." },
  chooseFolder: { en: "Browse…", id: "Telusuri…" },
  folderPickerMissing: {
    en: "Native folder picker is not registered in the backend yet — type or paste the path instead.",
    id: "Pemilih folder native belum terdaftar di backend — ketik atau tempel path saja.",
  },
  changesDiff: { en: "Diff", id: "Diff" },
  undoNotPossible: {
    en: "Undo not possible: file did not exist before (delete not implemented).",
    id: "Undo tidak bisa: file belum ada sebelumnya (hapus belum diimplementasikan).",
  },
  draftRestored: { en: "Your draft was kept.", id: "Draf Anda disimpan." },
  loading: { en: "Loading…", id: "Memuat…" },
};

export function t(lang: Lang, key: string): string {
  const entry = STRINGS[key];
  if (!entry) return key;
  return lang === "id" ? entry.id : entry.en;
}

/** t() with {0}, {1}… placeholders replaced (WinForms UiStrings.Format). */
export function tf(lang: Lang, key: string, ...args: (string | number)[]): string {
  let out = t(lang, key);
  args.forEach((a, i) => {
    out = out.replace(`{${i}}`, String(a));
  });
  return out;
}

// ---------------------------------------------------------------------------
// Reply-language detection (per message — the WinForms fix).
//
// Honest, simple heuristic: count strong Indonesian markers vs English
// markers in the user's question. Falls back to the UI language when the
// text is too short or ambiguous. This is a heuristic, not a language
// model — it is labelled as such in Settings.
// ---------------------------------------------------------------------------

const ID_MARKERS = [
  "yang", "dan", "atau", "dengan", "untuk", "dari", "saya", "aku", "kamu",
  "tolong", "bisakah", "bagaimana", "kenapa", "mengapa", "apakah", "berapa",
  "tolong", "buatkan", "tambahkan", "perbaiki", "jelaskan", "apakah",
  "ini", "itu", "sudah", "belum", "jangan", "bisa", "tidak", "nggak",
  "gak", "aja", "dong", "sih", "banget", "gimana", "kenapa", "kapan",
  "dimana", "di mana", "ke mana", "kemana", "siapa", "apa itu",
  "terima kasih", "maaf", "selamat", "pagi", "siang", "sore", "malam",
  "saya mau", "tolong buatkan", "coba", "lihat", "buka", "tutup",
  "saya ingin", "saya butuh", "saya punya", "lagi", "juga", "saja",
  "kalau", "jika", "karena", "supaya", "agar", "tetapi", "namun",
  "file", "folder", "aplikasi", "website", "halaman",
  // Greetings / short questions: "halo apa kabar ?" has no other marker,
  // so without these it ties 0-0 and falls back to the UI language.
  "halo", "hai", "kabar", "apa kabar", "apa",
];

const EN_MARKERS = [
  "the", "and", "with", "please", "could", "would", "should", "how",
  "what", "when", "where", "why", "which", "this", "that", "there",
  "please", "can you", "could you", "i want", "i need", "i have",
  "thank you", "thanks", "hello", "hi there", "good morning",
  "please add", "please fix", "please explain", "make a", "create a",
  "is there", "are there", "do you", "does it", "will it",
];

function countMarkers(text: string, markers: string[]): number {
  const lower = ` ${text.toLowerCase()} `;
  let n = 0;
  for (const m of markers) {
    if (m.includes(" ")) {
      if (lower.includes(` ${m} `) || lower.includes(` ${m}`)) n += 2;
    } else {
      const re = new RegExp(`\\b${m}\\b`, "g");
      const hits = lower.match(re);
      if (hits) n += hits.length;
    }
  }
  return n;
}

/** Returns "Bahasa Indonesia" or "English". */
export function detectReplyLanguage(
  text: string,
  followQuestion: boolean,
  uiLanguage: UiLanguage,
): "Bahasa Indonesia" | "English" {
  if (!followQuestion) {
    return uiLanguage === "id" ? "Bahasa Indonesia" : "English";
  }
  const trimmed = text.trim();
  if (trimmed.length < 3) {
    return uiLanguage === "id" ? "Bahasa Indonesia" : "English";
  }
  const idScore = countMarkers(trimmed, ID_MARKERS);
  const enScore = countMarkers(trimmed, EN_MARKERS);
  if (idScore > enScore) return "Bahasa Indonesia";
  if (enScore > idScore) return "English";
  // Tie / no markers: fall back to UI language.
  return uiLanguage === "id" ? "Bahasa Indonesia" : "English";
}
