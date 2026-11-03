//! Completion checks — port of `LocalTaskIntent.cs` and `WorkflowPolicy.cs`.
//!
//! This never executes or approves an operation; it only decides what
//! kind of local work a prompt requires, and whether a proposed action
//! batch is within the current request.
//!
//! NOTE on `Fold`: .NET folds with Unicode NFD + strips non-spacing
//! marks (via `System.Globalization`). The `unicode-normalization` crate
//! is not available offline, so this port folds with an explicit table of
//! precomposed Latin letters (Vietnamese + general Latin-1/Latin
//! Extended-A) to their ASCII base — the same characters the .NET
//! patterns rely on.

use regex::Regex;
use std::sync::OnceLock;

/// Fold diacritics the way .NET `Fold` does (NFD + drop combining marks),
/// approximated with a table of precomposed Latin letters.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        out.push(fold_char(ch));
    }
    out
}

fn fold_char(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ả' | 'ạ' | 'ă' | 'ắ' | 'ằ' | 'ẳ' | 'ẵ' | 'ặ' | 'ấ'
        | 'ầ' | 'ẩ' | 'ẫ' | 'ậ' | 'ä' | 'å' | 'ā' | 'ą' => 'a',
        'è' | 'é' | 'ê' | 'ẽ' | 'ẻ' | 'ẹ' | 'ế' | 'ề' | 'ể' | 'ễ' | 'ệ' | 'ë' | 'ē' | 'ĕ'
        | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ĩ' | 'ỉ' | 'ị' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ò' | 'ó' | 'ô' | 'õ' | 'ỏ' | 'ọ' | 'ơ' | 'ớ' | 'ờ' | 'ở' | 'ỡ' | 'ợ' | 'ố'
        | 'ồ' | 'ổ' | 'ỗ' | 'ộ' | 'ö' | 'ō' | 'ŏ' | 'ő' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ũ' | 'ủ' | 'ụ' | 'ư' | 'ứ' | 'ừ' | 'ử' | 'ữ' | 'ự' | 'ü' | 'ū'
        | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ỳ' | 'ý' | 'ŷ' | 'ỹ' | 'ỷ' | 'ỵ' | 'ÿ' => 'y',
        'đ' | 'ð' => 'd',
        'ñ' | 'ń' | 'ň' | 'ņ' | 'ŉ' => 'n',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
        'ź' | 'ż' | 'ž' => 'z',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ţ' | 'ť' | 'ŧ' | 'ț' => 't',
        _ => ch,
    }
}

/// Bitflags for the kind of local work (`.NET LocalWork`).
pub mod work {
    pub const NONE: u32 = 0;
    pub const READ: u32 = 1;
    pub const EDIT: u32 = 2;
    pub const COMMAND: u32 = 4;
    pub const BROWSE: u32 = 8;
    pub const COMPUTER: u32 = 16;
    pub const COMPUTER_INTERACT: u32 = 32;
    pub const COMPUTER_ASSERT: u32 = 64;
    pub const COMPUTER_CLICK: u32 = 128;
    pub const COMPUTER_INPUT: u32 = 256;
    pub const COMMAND_VERIFIED: u32 = 512;
    pub const COMPUTER_SCREENSHOT: u32 = 1024;

    pub const COMPUTER_WORK: u32 =
        COMPUTER | COMPUTER_INTERACT | COMPUTER_ASSERT | COMPUTER_CLICK | COMPUTER_INPUT | COMPUTER_SCREENSHOT;
}

fn re(pattern: &str) -> &'static Regex {
    // Compiled regexes are leaked (they live for the process lifetime), so
    // handing out `&'static` references is sound even as the cache grows.
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, &'static Regex>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(r) = guard.get(pattern) {
        return r;
    }
    let compiled: &'static Regex =
        Box::leak(Box::new(Regex::new(pattern).expect("intent regex")));
    guard.insert(pattern.to_string(), compiled);
    compiled
}

/// `WorkflowPolicy.IsThanksOrGreeting`
pub fn is_thanks_or_greeting(text: &str) -> bool {
    re(r"^(?:(?:cam on|thanks?|thank you|xin chao|chao|hello|hi)(?:\s+(?:ban|ni|nhe|nha|nhieu|a|oi|rat nhieu))*[.!,:\s]*)+$")
        .is_match(fold(text).trim())
}

/// `WorkflowPolicy.IsSmallTalk`
pub fn is_small_talk(text: &str) -> bool {
    is_thanks_or_greeting(text)
        || re(r"^(?:ok(?:ay)?|oke|uh|u|duoc|tot|hay|tuyet)(?:\s+(?:roi|do|nhe|nha|ni|a))*[.!\s]*$")
            .is_match(fold(text).trim())
}

/// `WorkflowPolicy.IsExplanation`
pub fn is_explanation(text: &str) -> bool {
    re(r"^(?:(?:hay|vui long|ban|ni)\s+)?(?:huong dan|lam the nao|giai thich|how to|show me how|co nen|theo ban)\b")
        .is_match(fold(text).trim())
}

/// `WorkflowPolicy.IsProjectExplanation`
pub fn is_project_explanation(text: &str) -> bool {
    let folded = fold(text);
    re(r"^(?:(?:hay|vui long|ban|ni)\s+)?giai thich\b").is_match(folded.trim())
        && re(r"\b(?:du an|project|repo|code|ma nguon|file|tep)\b").is_match(&folded)
}

/// `WorkflowPolicy.IsReadOnly`
pub fn is_read_only(text: &str) -> bool {
    let folded = fold(text);
    re(r"\bchi (?:can )?(?:doc|xem|tim hieu|phan tich|giai thich|review)\b").is_match(&folded)
        && !re(r"\b(?:roi|sau do|va) (?:hay )?(?:sua|chay|thuc hien|cap nhat)\b").is_match(&folded)
}

/// `WorkflowPolicy.IsContinue`
pub fn is_continue(text: &str) -> bool {
    re(r"^(?:(?:hay|ban|ni)\s+)?(?:tiep tuc|continue|resume)(?:\s+(?:di|nhe|nha|ni|goal|cho den khi xong))*[.!\s]*$")
        .is_match(fold(text).trim())
}

/// `WorkflowPolicy.IsPermissionQuestion`
pub fn is_permission_question(text: &str) -> bool {
    re(r"\b(?:ban (?:co )?muon (?:minh|toi) (?:thuc hien|chay|sua|viet|soan|tiep tuc|ap dung)|cho phep (?:minh|toi) (?:chay|sua|tiep tuc)|may i (?:run|edit|implement|continue))\b")
        .is_match(&fold(text))
}

/// `WorkflowPolicy.Allows`: is this tool call within the current request?
pub fn allows(prompt: &str, tool: &str, goal: bool) -> bool {
    if goal && !is_read_only(prompt) {
        return true;
    }
    if is_small_talk(prompt) {
        return false;
    }
    if is_explanation(prompt) {
        return is_project_explanation(prompt)
            && matches!(
                tool,
                "read_file"
                    | "read_files"
                    | "search_files"
                    | "list_files"
                    | "git_status"
                    | "git_diff"
            );
    }
    let required = required(prompt);
    let diagnostic_read = required == work::READ
        && re(r"\b(?:tai sao|vi sao|nguyen nhan|why)\b").is_match(&fold(prompt));
    if is_read_only(prompt) || diagnostic_read {
        return matches!(
            tool,
            "read_file"
                | "read_files"
                | "search_files"
                | "list_files"
                | "git_status"
                | "git_diff"
        ) || (required & work::COMPUTER != 0
            && matches!(
                tool,
                "computer_inspect" | "computer_screenshot" | "computer_assert"
            ));
    }
    true
}

/// `LocalTaskIntent.Required`: what local work does this prompt require?
pub fn required(prompt: &str) -> u32 {
    let text = fold(prompt);
    if is_project_explanation(prompt) {
        return work::READ;
    }
    if is_small_talk(prompt) || is_explanation(prompt) {
        return work::NONE;
    }
    if re(r"^(?:hay |vui long )?(?:huong dan|lam the nao|giai thich cach|how to|show me how)\b")
        .is_match(&text)
    {
        return work::NONE;
    }
    if re(r"\b(?:tai sao|vi sao|nguyen nhan|why)\b").is_match(&text)
        && re(r"\b(?:code|ma nguon|project|repo)\b").is_match(&text)
        && !re(r"\b(?:sua|chinh sua|fix|implement)\b").is_match(&text)
    {
        return work::READ;
    }
    let mut result = work::NONE;
    if re(r"\b(?:kiem tra|danh gia|xem) (?:muc )?do hoan thien\b").is_match(&text) {
        result |= work::READ;
    }
    let edit_text = re(r"\b(?:muc )?do hoan thien\b").replace_all(&text, "muc do day du");
    if re(r"\b(doc|xem (?:file|tep|du an|ma|git)|tim (?:file|tep|loi|kiem)|kiem tra (?:du an|ma|thay doi|code)|tim hieu du an|giai thich du an|read|inspect|search files|review project)\b")
        .is_match(&text)
    {
        result |= work::READ;
    }
    if re(r"\b(liet ke|cau truc|list files|list directory)\b").is_match(&text) {
        result |= work::BROWSE;
    }
    if re(r"\b(sua|chinh sua|cap nhat (?:file|tep|ma|ui)|tao (?:file|tep)|them (?:tinh nang|chuc nang|nut|che do|dark|light)|doi (?:theme|mau|giao dien)|toi uu|cai thien|hoan thien|thiet ke lai|nang cap|phat trien|edit|modify|replace|fix|implement|optimize|create (?:a )?file|add (?:a )?(?:button|feature|dark|light|theme))\b")
        .is_match(&edit_text)
    {
        result |= work::EDIT;
    }
    if re(r"\b(?:toi|minh) muon (?:phan cai dat|giao dien|theme|dark|light)\b").is_match(&text) {
        result |= work::EDIT;
    }
    if requests_command(&text) {
        result |= work::COMMAND;
    }
    if re(r"\b(?:hay |ban )?(?:thuc hien|lam)(?: luon| di| ngay| giup| cho minh| cho toi)+[.! ]*$")
        .is_match(&text)
    {
        result |= work::EDIT;
    }
    if re(r"\b(dieu khien (?:man hinh|cua so)|kiem thu giao dien|test ui|test giao dien|click|bam nut|chup (?:man hinh|cua so))\b")
        .is_match(&text)
    {
        result |= work::COMPUTER;
    }
    if re(r"\b(kiem thu giao dien|test ui|test giao dien)\b").is_match(&text) {
        result |= work::COMPUTER_ASSERT;
    }
    if re(r"\bchup (?:man hinh|cua so)\b").is_match(&text) {
        result |= work::COMPUTER_SCREENSHOT;
    }
    if re(r"\b(click|bam nut)\b").is_match(&text) {
        result |= work::COMPUTER_CLICK;
    }
    if re(r"\b(cuon (?:xuong|len|sang|trai|phai)|keo tha|bam (?:chuot phai|dup)|nhap dup|doc (?:giao dien|man hinh|cua so)|(?:nhap|dien|go)\b[^\n.!?]{0,100}\b(?:vao o|textbox|truong nhap))\b")
        .is_match(&text)
    {
        result |= work::COMPUTER;
    }
    if re(r"\b(?:keo tha|cuon (?:xuong|len|sang|trai|phai))\b").is_match(&text) {
        result |= work::COMPUTER_INTERACT;
    }
    if re(r"\b(?:bam (?:chuot phai|dup)|nhap dup)\b").is_match(&text) {
        result |= work::COMPUTER_CLICK;
    }
    if result & work::COMPUTER_WORK != 0
        && re(r"\b(nhap|dien|go)\b.*\b(o|input|textbox|truong)\b").is_match(&text)
    {
        result |= work::COMPUTER_INPUT;
    }
    if result & work::COMMAND != 0
        && re(r"\b(kiem tra|kiem thu|tests?|build|bien dich)\b").is_match(&text)
    {
        result |= work::COMMAND_VERIFIED;
    }
    if result & work::COMPUTER_WORK != 0 {
        let explicit_project_read = re(r"\b(?:doc|xem|read|inspect|tim|kiem tra)\b[^\n!?]{0,60}\b(?:file|tep|du an|project|ma nguon|code|thu muc|repository)\b")
            .is_match(&text)
            || re(r"\b(?:doc|read)\b[^\n!?]{0,60}\.[a-z0-9]{1,8}\b").is_match(&text);
        if !explicit_project_read {
            result &= !work::READ;
        }
        if !re(r"\b(?:liet ke (?:file|tep|thu muc)|list files|list directory)\b").is_match(&text) {
            result &= !work::BROWSE;
        }
        if re(r"\b(?:kiem thu giao dien|test ui|test giao dien)\b").is_match(&text)
            && !re(r"\b(?:cmd|powershell|terminal|dotnet|npm|pnpm|yarn|python|bash|lenh|shell)\b")
                .is_match(&text)
        {
            result &= !(work::COMMAND | work::COMMAND_VERIFIED);
        }
    }
    if is_read_only(prompt) {
        result &= !(work::EDIT
            | work::COMMAND
            | work::COMMAND_VERIFIED
            | work::COMPUTER_INTERACT
            | work::COMPUTER_CLICK
            | work::COMPUTER_INPUT);
        if result == work::NONE {
            result = work::READ;
        }
    }
    result
}

fn requests_command(text: &str) -> bool {
    for m in re(r"\b(chay|thuc thi|run|execute)\b").find_iter(text) {
        let before = &text[..m.start()];
        let after = &text[m.end()..];
        let clause = before
            .split(|c| matches!(c, '\r' | '\n' | ',' | ';' | '.' | '!' | '?'))
            .last()
            .unwrap_or("")
            .trim();
        let tail = after
            .split(|c| matches!(c, '\r' | '\n' | ';' | '!' | '?'))
            .next()
            .unwrap_or("");
        if re(r"\b(dung|cho|khong|chua|do not|don't|never)(?: can)?(?: tu)?(?: dong)?$")
            .is_match(clause)
            || re(r"^.*\b(co can|tai sao|vi sao|nghia la gi|de lam gi|why|what happens)\b")
                .is_match(tail)
        {
            continue;
        }
        if clause.is_empty()
            || re(r"\b(hay|vui long|lam on|nho|please|then|roi|sau do|va)$").is_match(clause)
            || re(r"^(?:ban|ni)$|\b(?:(?:toi|tui|minh) (?:muon|can)|(?:ban|ni) (?:hay|co the)|(?:giup|cho) (?:toi|tui|minh))(?: (?:ban|ni))?$")
                .is_match(clause)
            || re(r"\b(?:giup|cho|ho) (?:toi|tui|minh)\b").is_match(tail)
        {
            return true;
        }
    }
    false
}

/// `LocalTaskIntent.Evidence`: what work does a successful tool call prove?
pub fn evidence(tool: &str) -> u32 {
    match tool {
        "read_file" | "read_files" | "search_files" | "git_diff" => work::READ,
        "list_files" | "git_status" => work::BROWSE,
        "write_file" | "edit_file" => work::EDIT,
        "run_command" | "process_status" => work::COMMAND,
        "computer_inspect" => work::COMPUTER,
        "computer_screenshot" => work::COMPUTER | work::COMPUTER_SCREENSHOT,
        "computer_assert" => work::COMPUTER | work::COMPUTER_ASSERT,
        "computer_invoke" | "computer_click" | "computer_toggle" | "computer_select"
        | "computer_expand" => work::COMPUTER_INTERACT | work::COMPUTER_CLICK,
        "computer_set_value" | "computer_type" => work::COMPUTER_INTERACT | work::COMPUTER_INPUT,
        "computer_key" | "computer_scroll" | "computer_drag" | "computer_focus" => {
            work::COMPUTER | work::COMPUTER_INTERACT
        }
        _ => work::NONE,
    }
}

/// `LocalTaskIntent.IsEnvironmentConfusion`
pub fn is_environment_confusion(text: &str) -> bool {
    let value = fold(text);
    re(r"\b(vm linux|may ao|tren web|web muse|muse tren web|ban windows|ban copy tren vm)\b")
        .is_match(&value)
        && re(r"\b(khong (?:the|tu|truy cap|sua|phai)|chua sua|ban dan|chi la chu|push|gui (?:file|noi dung))\b")
            .is_match(&value)
}

/// `LocalTaskIntent.IsClarification`
pub fn is_clarification(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with('?')
        && !is_environment_confusion(text)
        && !re(r"\b(da (?:sua|chay|doc|tao)|completed|executed|modified|neu ban muon|ban (?:co )?muon (?:minh )?(?:thuc hien|bam|chay|sua|viet|soan|tiep tuc|ap dung))\b")
            .is_match(&fold(text))
}

/// `LocalTaskIntent.ExplainsNoChange`
pub fn explains_no_change(text: &str) -> bool {
    re(r"\b(khong can (?:sua|thay doi)|khong co thay doi|noi dung da khop|giu nguyen|no changes? needed|already (?:matches|correct))\b")
        .is_match(&fold(text))
}

/// Human-readable rendering of a `LocalWork` bitmask, for the APP_STATE
/// packet (`.NET LocalWork.ToString()` prints flag names joined by ", ").
pub fn work_to_string(flags: u32) -> String {
    if flags == work::NONE {
        return "None".to_string();
    }
    let names = [
        (work::READ, "Read"),
        (work::EDIT, "Edit"),
        (work::COMMAND, "Command"),
        (work::BROWSE, "Browse"),
        (work::COMPUTER, "Computer"),
        (work::COMPUTER_INTERACT, "ComputerInteract"),
        (work::COMPUTER_ASSERT, "ComputerAssert"),
        (work::COMPUTER_CLICK, "ComputerClick"),
        (work::COMPUTER_INPUT, "ComputerInput"),
        (work::COMMAND_VERIFIED, "CommandVerified"),
        (work::COMPUTER_SCREENSHOT, "ComputerScreenshot"),
    ];
    let parts: Vec<&str> = names
        .iter()
        .filter(|(flag, _)| flags & flag != 0)
        .map(|(_, name)| *name)
        .collect();
    parts.join(", ")
}
