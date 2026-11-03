fn main() {
    // Perbaikan 3 Nov 2026: rebuild 10:20/11:01/11:44 menanam icon
    // LAMA karena `resource.lib` basi (dikompilasi 2 Nov, sebelum
    // regenerasi icon 3 Nov 06:20) — cargo tidak me-re-run build
    // script karena `resource.rc` tidak berubah isinya. Paksa
    // re-run tiap icon berubah supaya resource.lib tidak basi lagi.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
