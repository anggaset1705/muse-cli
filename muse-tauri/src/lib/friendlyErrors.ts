// Friendly error mapping — port of DesktopCopy.FriendlyError
// (~/workspace/muse-cli-en-id/src/Muse2Api.WinForms/DesktopCopy.cs:5-20)
// with the Friendly* strings from UiStrings.cs (EN + ID). The raw
// gateway/HTTP text is matched case-insensitively; the user only ever
// sees the friendly sentence. The raw text is kept in the caller for
// diagnostics (never shown as the main message).

import type { Lang } from "./i18n";

const FRIENDLY: Record<string, { en: string; id: string }> = {
  unavailable: {
    en: "The app could not read the completed reply from Muse web. It stopped waiting and kept the current chat; it did not resend. This is not an account quota message; check the Terminal before continuing.",
    id: "Aplikasi belum bisa membaca balasan selesai dari Muse web. Sudah berhenti menunggu dan menyimpan chat saat ini, tidak mengirim ulang. Ini bukan pesan habis kuota; periksa Terminal sebelum melanjutkan.",
  },
  webStopped: {
    en: "Muse web stopped responding or lost its connection. The app stopped waiting and kept the current chat; it did not resend the request. Check the Terminal before continuing.",
    id: "Muse web berhenti merespons atau kehilangan koneksi. Aplikasi sudah berhenti menunggu dan menyimpan chat saat ini; tidak mengirim ulang permintaan. Periksa Terminal sebelum melanjutkan.",
  },
  rateLimit: {
    en: "Muse is rate-limiting requests. Sending stopped and the current chat was kept. Wait and try again; no need to sign in again.",
    id: "Muse membatasi frekuensi permintaan. Pengiriman dihentikan dan chat saat ini disimpan. Tunggu lalu coba lagi; tidak perlu masuk lagi.",
  },
  reconnect: {
    en: "The internal connection needs to be refreshed. Click Connect Muse and the app will restore it.",
    id: "Koneksi internal perlu disegarkan. Klik Hubungkan Muse dan aplikasi akan memulihkannya.",
  },
  sessionExpired: {
    en: "Your Muse sign-in session expired. Click your Muse account to sign in again.",
    id: "Sesi login Muse Anda sudah kedaluwarsa. Klik akun Muse Anda untuk masuk lagi.",
  },
  quota: {
    en: "Your Muse account is out of turns. You can try again later or switch accounts.",
    id: "Akun Muse Anda sudah habis jatah. Anda bisa coba lagi nanti atau ganti akun.",
  },
  cooldown: {
    en: "Muse accounts are waiting for a processing turn. Your sign-in is still saved; try again later, no need to get a new code.",
    id: "Akun Muse sedang menunggu giliran pemrosesan. Login Anda tetap tersimpan; coba lagi nanti, tidak perlu mengambil kode baru.",
  },
  timeout: {
    en: "Stopped waiting because Muse took too long to return a result. The chat was kept and the app did not resend; check the Terminal before continuing.",
    id: "Berhenti menunggu karena Muse terlalu lama mengembalikan hasil. Chat disimpan dan aplikasi tidak mengirim ulang; periksa Terminal sebelum melanjutkan.",
  },
  connection: {
    en: "The Muse connection is interrupted. Click Connect Muse to try again.",
    id: "Koneksi Muse sedang terputus. Klik Hubungkan Muse untuk mencoba lagi.",
  },
  fileTooBig: {
    en: "This file is too large or is not a text file. Choose a smaller source file.",
    id: "File ini terlalu besar atau bukan file teks. Pilih file kode sumber yang lebih kecil.",
  },
  outsideProject: {
    en: "The selected file is outside the project or is a shortcut. Choose a file directly inside the project folder.",
    id: "File yang dipilih berada di luar proyek atau berupa pintasan. Pilih file langsung di dalam folder proyek.",
  },
  generic: {
    en: "The action could not be done. You can try again; details are in the Activity panel.",
    id: "Tindakan tidak bisa dilakukan. Anda bisa mencoba lagi; detail ada di panel Aktivitas.",
  },
};

/** Port of DesktopCopy.FriendlyError: same pattern order as WinForms. */
export function friendlyError(message: string, lang: Lang): string {
  const text = message.toLowerCase();
  const pick = (k: keyof typeof FRIENDLY) => (lang === "id" ? FRIENDLY[k].id : FRIENDLY[k].en);
  if (text.includes("upstream_response_unavailable")) return pick("unavailable");
  if (
    text.includes("stopped making progress") ||
    text.includes("web state unavailable") ||
    text.includes("web stopped responding") ||
    text.includes("web page closed")
  )
    return pick("webStopped");
  if (text.includes("rate limit") || text.includes("too many requests")) return pick("rateLimit");
  if (text.includes("401") || text.includes("api key")) return pick("reconnect");
  if (text.includes("upstream_auth") || text.includes("login") || text.includes("session rejected"))
    return pick("sessionExpired");
  if (text.includes("429") || text.includes("quota") || text.includes("credits")) return pick("quota");
  if (text.includes("available account") || text.includes("no usable account") || text.includes("cooldown"))
    return pick("cooldown");
  if (text.includes("timeout") || text.includes("timed out")) return pick("timeout");
  if (text.includes("connection") || text.includes("connect") || text.includes("503")) return pick("connection");
  if (text.includes("binary") || text.includes("2 mib")) return pick("fileTooBig");
  if (text.includes("outside") || text.includes("junction") || text.includes("symbolic")) return pick("outsideProject");
  return pick("generic");
}
