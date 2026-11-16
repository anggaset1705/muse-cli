//! Syntax and basic readability gate for source produced by Muse Agent.
//! Reject malformed writes before touching files and before tool_content.
//! Never execute generated source code here.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const PYTHON_PARSE: &str = r#"
import ast
import sys

source = sys.stdin.read()
try:
    tree = ast.parse(source, filename=sys.argv[1])
except SyntaxError as exc:
    print(f"Python syntax, line {exc.lineno}: {exc.msg}", file=sys.stderr)
    sys.exit(2)

# Syntactically legal semicolon-separated statements and one-line
# control bodies are not appropriate for readable generated files.
lines = {}
for node in ast.walk(tree):
    if isinstance(node, ast.stmt) and hasattr(node, "lineno"):
        line = node.lineno
        if line in lines:
            print(f"Python formatting, line {line}: more than one statement on one line. Put each statement on its own line with proper indentation.", file=sys.stderr)
            sys.exit(3)
        lines[line] = True
"#;

const SOURCE_EXTENSIONS: &[&str] = &[
    "py", "pyw", "pyi", "js", "mjs", "cjs", "jsx", "ts", "tsx",
    "rs", "go", "java", "kt", "kts", "c", "h", "hpp", "cpp",
    "cs", "swift", "rb", "php", "lua", "sh", "bash", "ps1",
    "html", "htm", "css", "scss", "vue", "svelte", "sql",
];

async fn check_with_stdin(
    command: &str,
    args: &[&str],
    content: &str,
) -> Result<Option<String>, String> {
    let mut process = Command::new(command);
    process
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("unable to start code parser {command}: {err}")),
    };
    let result = tokio::time::timeout(Duration::from_secs(8), async move {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(content.as_bytes())
                .await
                .map_err(|err| format!("cannot send source to parser: {err}"))?;
        }
        child
            .wait_with_output()
            .await
            .map_err(|err| format!("code parser failed to finish: {err}"))
    })
    .await
    .map_err(|_| format!("code parser {command} timed out"))??;

    if result.status.success() {
        Ok(Some(String::new()))
    } else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let msg = stderr.trim();
        let detail = if msg.is_empty() {
            "parser returned a nonzero status".to_string()
        } else {
            msg.chars().take(1200).collect::<String>()
        };
        Err(detail)
    }
}

/// Validate completed source without executing its contents.
/// Every known source extension gets a minimal real-newline check. Python
/// additionally uses CPython ast.parse, JS uses Node parser, JSON uses serde.
pub async fn validate(path: &Path, source: &str) -> Result<(), String> {
    let filename = path.file_name().and_then(|x| x.to_str()).unwrap_or("");
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if SOURCE_EXTENSIONS.contains(&ext.as_str())
        && !filename.to_ascii_lowercase().contains(".min.")
        && source.chars().count() > 250
        && source.lines().count() < 3
    {
        return Err(format!(
            "Code formatting: {filename} is flattened into fewer than three source lines. Regenerate readable source with real newlines, separate statements, and normal indentation. The file was NOT written."
        ));
    }

    match ext.as_str() {
        "py" | "pyw" | "pyi" => {
            let path_str = path.to_string_lossy();
            let interpreters: &[&str] = if cfg!(windows) {
                &["python", "python3"]
            } else {
                &["python3", "python"]
            };
            for interpreter in interpreters {
                match check_with_stdin(
                    interpreter,
                    &["-I", "-c", PYTHON_PARSE, path_str.as_ref()],
                    source,
                )
                .await
                {
                    Ok(Some(_)) => return Ok(()),
                    Ok(None) => continue,
                    Err(reason) => {
                        return Err(format!(
                            "Code formatting/syntax rejected for {filename}: {reason}. Regenerate valid Python code with separate lines and four-space indentation. File was NOT written."
                        ))
                    }
                }
            }
            Err(
                "Cannot validate Python source: Python 3 is not available. Install Python to safely create .py files; file was NOT written.".to_string()
            )
        }
        "json" => serde_json::from_str::<Value>(source)
            .map(|_| ())
            .map_err(|err| format!("Invalid JSON syntax in {filename}: {err}. File was NOT written.")),
        "js" | "mjs" | "cjs" => {
            let options: &[&str] = if ext == "cjs" {
                &["--check"]
            } else {
                &["--input-type=module", "--check"]
            };
            match check_with_stdin("node", options, source).await {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err("Cannot validate JavaScript: Node.js is not available. File was NOT written.".to_string()),
                Err(detail) => Err(format!(
                    "Invalid JavaScript syntax in {filename}: {detail}. Regenerate proper multi-line source. File was NOT written."
                )),
            }
        }
        _ => Ok(()),
    }
}
