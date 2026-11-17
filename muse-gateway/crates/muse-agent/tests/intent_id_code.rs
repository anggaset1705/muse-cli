//! Regression from 14 November Indonesian Muse CLI screenshot:
//! "buatkan script python kalkulator dengan ui style neumorphism"
//! must not complete with "the file already exists" and no file operation.
use muse_agent::intent::{self, work};

#[test]
fn indonesian_requests_to_create_source_require_file_work() {
    for input in [
        "buatkan script python kalkulator dengan ui style neumorphism",
        "buatkan script python kalkulator dengan ui style neumorphis",
        "buatkan kode TypeScript untuk login",
        "tolong buat file kalkulator_neumorphism.py",
        "bikin program kalkulator Python",
        "tolong tuliskan source code Go sebagai file",
        "tolong buatkan aplikasi React dari awal",
        "buat source Rust untuk program kecil",
        "buatkan file index.html untuk landing page",
    ] {
        assert_ne!(
            intent::required(input) & work::EDIT,
            0,
            "Explicit code creation requested but file work not required: {input:?}"
        );
    }
}

#[test]
fn indonesian_discussion_questions_do_not_trigger_file_creation() {
    for input in [
        "apa itu script Python?",
        "apakah bisa buatkan script Python?",
        "bagaimana cara buat kalkulator dengan python?",
        "tolong jelaskan cara membuat script Python",
        "berapa baris kode yang ideal?",
        "ini code Python salahnya di mana?",
    ] {
        assert_eq!(
            intent::required(input) & work::EDIT,
            0,
            "Do not change files for an informational question: {input:?}"
        );
    }
}

#[test]
fn existing_non_indonesian_policies_remain_unchanged() {
    assert_eq!(intent::required("hello"), work::NONE);
    assert_ne!(intent::required("create a file test.txt") & work::EDIT, 0);
    assert_ne!(intent::required("edit the file main.rs") & work::EDIT, 0);
    assert_eq!(intent::required("how to write a Rust program?") & work::EDIT, 0);
}


#[test]
fn general_code_format_rules_cover_every_code_card() {
    let prompt = muse_agent::system_prompt::system_prompt_for("Bahasa Indonesia");
    assert!(prompt.contains("normal chat code fences"));
    assert!(prompt.contains("every code card"));
    assert!(prompt.contains("write_file/edit_file"));
    assert!(prompt.contains("separate statements/blocks on separate"));
}


#[test]
fn indonesian_existing_file_edits_must_execute_not_just_show_code() {
    for prompt in [
        "pada kalkulator_sederhana.py ubah tampilan tombol sama dengan (=) menjadi lebih profesional, gunakan warna biru dan efek hover. Jangan mengubah fungsi kalkulator lainnya.",
        "perbaiki UI kalkulator_sederhana.py dan gunakan ui style neumorphism yang profesional",
        "tolong ganti warna tombol dalam file main.tsx",
        "rapikan CSS tampilan aplikasi supaya profesional",
        "tambahkan efek hover pada tombol kalkulator.py",
        "hapus fungsi lama dari file main.rs",
        "ubah tampilan tombol saja, fungsi yang lain jangan diubah",
    ] {
        assert_ne!(
            intent::required(prompt) & work::EDIT,
            0,
            "Existing-file modification MUST require real edit_file/write_file, not prose: {prompt}",
        );
    }
}

#[test]
fn indonesian_negative_edit_constraints_and_questions_must_not_write() {
    for prompt in [
        "jangan mengubah fungsi kalkulator_sederhana.py",
        "tidak usah ganti warna di file main.css",
        "tanpa mengubah kode script.py",
        "apa yang harus diubah di kalkulator_sederhana.py?",
        "bagaimana cara mengubah tampilan UI?",
        "tolong jelaskan cara memperbaiki file Python",
        "apakah bisa perbaiki UI kalkulator_sederhana.py?",
    ] {
        assert_eq!(
            intent::required(prompt) & work::EDIT,
            0,
            "Negative constraint or advice question must not trigger native write: {prompt}",
        );
    }
}


#[test]
fn single_filename_native_read_preflight_only_for_explicit_edit_request() {
    use muse_agent::intent::unambiguous_edit_filename;
    assert_eq!(
        unambiguous_edit_filename("ubah tombol kalkulator_sederhana.py menjadi warna biru semua"),
        Some("kalkulator_sederhana.py".to_string()),
    );
    assert_eq!(
        unambiguous_edit_filename("perbaiki UI kalkulator_sederhana.py dan jangan mengubah rumus"),
        Some("kalkulator_sederhana.py".to_string()),
    );
    assert_eq!(
        unambiguous_edit_filename("halo apa kabar?"),
        None,
    );
    assert_eq!(
        unambiguous_edit_filename("apa fungsi kalkulator_sederhana.py?"),
        None,
    );
    assert_eq!(
        unambiguous_edit_filename("jangan mengubah kalkulator_sederhana.py"),
        None,
    );
    assert_eq!(
        unambiguous_edit_filename("ubah a.py dan b.py agar sama"),
        None,
    );
    assert_eq!(
        unambiguous_edit_filename("buatkan file baru test.py"),
        Some("test.py".to_string()),
    );
}
