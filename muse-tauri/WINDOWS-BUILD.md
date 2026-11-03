# Build Windows — Muse Tauri (v0.1.0)

> BELUM dicoba di Windows setelah pergantian sidecar ke Rust.
> Instruksi ini disusun dari konfigurasi `tauri.conf.json` +
> `src-tauri/` yang sudah divalidasi di Linux (build Linux lolos)
> dan dari cara `start_backend` di Rust mencari sidecar. Jangan
> klaim build Windows berhasil sebelum dijalankan di PC.

## Layout repo

Repo `anggaset1705/muse-cli`:

```
muse-cli/
  muse-tauri/    <- app ini (frontend + Tauri)
  muse-gateway/  <- workspace backend Rust (biner muse-gateway)
```

## Prasyarat (PC Windows Bree)
1. **Rust** — instal via rustup: https://rustup.rs (pilih MSVC toolchain).
   Cek: `cargo --version` dan `rustc --version`.
2. **Node.js 24** (LTS) + npm — https://nodejs.org. Cek: `node --version`.
3. **WebView2** — sudah ada di Windows 10/11 modern (Tauri v2 butuh itu).
4. **Visual Studio Build Tools (MSVC)** — dibutuhkan Rust untuk link
   native di Windows (rustup biasanya mengarahkan ke sini).
5. **Git** — untuk clone repo.

.NET TIDAK dibutuhkan lagi: backend sekarang murni Rust
(`muse-gateway.exe`); tidak ada lagi fallback `Muse2Api.dll` /
`dotnet run`.

## Langkah build
```powershell
# 1. Clone repo (satu repo berisi app + gateway)
git clone https://github.com/anggaset1705/muse-cli.git
cd muse-cli\muse-tauri

# 2. Dependensi frontend
npm install

# 3. Build gateway Rust + salin muse-gateway.exe ke samping exe Tauri
#    (script ini: cargo build --release -p muse-gateway di ../muse-gateway,
#    lalu copy hasilnya ke src-tauri\target\release\)
npm run build:gateway

# 4. Build frontend + Tauri (hasilkan .exe + installer)
npm run tauri build
```
Hasil: `src-tauri\target\release\muse-tauri.exe` (+ installer
`.msi` di `src-tauri\target\release\bundle\msi\`) dan
`src-tauri\target\release\muse-gateway.exe` di sampingnya.

Urutan penting: gateway dulu (langkah 3), baru Tauri (langkah 4).
Tanpa `muse-gateway.exe` di samping exe, app jalan tapi backend
tidak bisa start — dialog login akan bilang gateway tidak ditemukan.

## Sidecar Rust (muse-gateway.exe) — WAJIB, kalau tidak backend tidak jalan
Aplikasi Tauri mencari backend dengan urutan ini (`src-tauri/src/lib.rs`,
`resolve_backend_path`):
1. Env `MUSE_BACKEND_PATH` — path ke executable `muse-gateway`
   (`.exe` di Windows).
2. `<folder exe>\muse-gateway.exe` (rilis yang dipaket — ini yang
   disiapkan `npm run build:gateway`).
3. (Dev) `<repo>\muse-gateway\target\release\muse-gateway.exe`.

Tidak ada fallback lain: kalau tidak ketemu di ketiga tempat itu,
`start_backend` gagal dengan pesan jelas (`muse-gateway tidak
ditemukan...` + daftar lokasi yang dicek) — bukan fallback diam-diam.

Cara manual (tanpa script), dari PowerShell di root repo:
```powershell
cargo build --release -p muse-gateway --manifest-path muse-gateway\Cargo.toml
Copy-Item muse-gateway\target\release\muse-gateway.exe muse-tauri\src-tauri\target\release\
# Opsi lain: set env permanen
setx MUSE_BACKEND_PATH "C:\muse-cli\muse-gateway\target\release\muse-gateway.exe"
```
Sidecar dijalankan langsung sebagai `muse-gateway.exe` oleh Rust
(`start_backend`), listen di `127.0.0.1` port acak, env
`MUSE2API_HOST=127.0.0.1` di-set otomatis. Data gateway (akun, sesi
agen, skill, preferensi) tinggal di `<folder exe>\Muse2Api\data` —
nama folder dipertahankan dari era lama, isinya data gateway Rust.

## Login akun nyata (di PC Bree, oleh Bree sendiri)
1. Jalankan `muse-tauri.exe` → sidebar → **Login**.
2. Klik **Start backend** bila belum jalan (tombol darurat di strip
   kuning / ikon sidebar), lalu **Sign in inside the app** — jendela
   sign-in Muse terbuka di dalam app (profil WebView2 terpisah).
3. Masuk dengan akun Muse di jendela itu; app menyimpan akun otomatis.
   Tidak ada tempel cookie manual — jalur itu sudah dibuang.
4. Verifikasi nyata = kirim satu chat dan lihat balasan masuk.

## Uji setelah build Windows
- `cargo test` di `muse-gateway` (workspace Rust) — lihat
  `muse-gateway/VERIFY.md` untuk daftar cek yang pernah lolos di VM.
- Jalankan exe → backend healthy → kirim chat "halo" (mode Ask) →
  bila Muse mengusulkan tulis file, dialog Approve/Deny harus muncul.
- Mode Turbo: hanya bila Bree menginginkannya (auto-approve).
- Screen tab: kontrol layar lewat `/v1/agent/screen/*` gateway
  (UIA di Windows) — nyata, bukan stub.

## Validasi konfigurasi (sudah dicek di Linux)
- `tauri.conf.json`: id `com.bstore.muse`, productName `Muse`,
  version `0.1.0`, window 1560×900 (min 1100×650), app windows
  `app > windows` — tidak ada pengaturan khusus Windows yang hilang;
  build Windows memakai `bundle > active: true` default (msi + nsis).
- Sidecar external binary TIDAK dideklarasikan di `tauri.conf.json`
  (`bundle > externalBin` kosong) — sengaja: sidecar dicari di runtime
  lewat `MUSE_BACKEND_PATH` / folder di samping exe, bukan di-bundle
  sebagai `externalBin`. Kalau mau dibundle otomatis, tambahkan
  `bundle.externalBin` + letakkan exe di `src-tauri/binaries/` —
  belum dilakukan, karena penamaan biner sidecar per-target
  (`muse-gateway-x86_64-pc-windows-msvc.exe`) belum diuji.
- Cross-compile Windows dari Linux TIDAK dicoba (butuh toolchain
  `x86_64-pc-windows-msvc` + linker `lld` + WebView2 SDK — di luar
  lingkup VM ini).

## Build Linux (VM — untuk validasi, bukan target rilis)
Urutan yang sama, dari root repo:
```bash
cd muse-cli/muse-tauri
npm install
npm run build:gateway   # biner: muse-gateway (tanpa .exe)
npm run tauri build     # hasil: src-tauri/target/release/muse-tauri
```
