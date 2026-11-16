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
