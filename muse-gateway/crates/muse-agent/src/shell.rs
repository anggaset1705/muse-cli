//! Shell resolution + invocation — port of `Tools.ResolveShell` and
//! `ToolProcess.CreateShellInvocation`.
//!
//! On Linux/macOS the shell is always `/bin/bash -c <command>` (the .NET
//! agent runs on this VM the same way). `powershell` / `cmd` are
//! Windows-only and rejected elsewhere, exactly like .NET. On Windows,
//! PowerShell/cmd commands are written to a private temp script file so
//! multiline Unicode/quotes survive intact.

use std::path::PathBuf;

use crate::error::{AgentError, AgentResult};

/// A prepared shell invocation. `temp_dir`, when present, holds a private
/// script file (Windows PowerShell/cmd) and must be removed after the run.
pub struct ShellInvocation {
    pub file: String,
    pub args: Vec<String>,
    pub temp_dir: Option<PathBuf>,
}

impl ShellInvocation {
    pub fn cleanup(&self) {
        if let Some(dir) = &self.temp_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Resolve `(file, args)` for a command under the requested shell
/// (`.NET ResolveShell`).
pub fn resolve_shell(requested: &str, configured: &str, command: &str) -> AgentResult<(String, Vec<String>)> {
    let mut want = if requested.trim().is_empty() {
        configured.to_string()
    } else {
        requested.to_string()
    };
    want = want.trim().to_lowercase();
    want = match want.as_str() {
        "cmd.exe" => "cmd".to_string(),
        "powershell.exe" => "powershell".to_string(),
        "bash.exe" => "bash".to_string(),
        other => other.to_string(),
    };
    if !matches!(want.as_str(), "auto" | "powershell" | "cmd" | "bash") {
        return Err(AgentError::Invalid(
            "shell must be auto, powershell, cmd or bash".to_string(),
        ));
    }
    #[cfg(windows)]
    {
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        if want == "cmd" {
            return Ok((
                format!("{system}\\System32\\cmd.exe"),
                vec!["/d".to_string(), "/c".to_string(), command.to_string()],
            ));
        }
        if want == "bash" {
            return Ok(("bash".to_string(), vec!["-c".to_string(), command.to_string()]));
        }
        // Default on Windows: PowerShell, like Codex on Windows.
        return Ok((
            format!("{system}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        ));
    }
    #[cfg(not(windows))]
    {
        if want == "powershell" || want == "cmd" {
            return Err(AgentError::Invalid(format!(
                "shell '{want}' is only supported on Windows"
            )));
        }
        // Linux/macOS (used to develop and test this agent on this VM).
        Ok((
            "/bin/bash".to_string(),
            vec!["-c".to_string(), command.to_string()],
        ))
    }
}

/// Build the invocation for a command (`.NET CreateShellInvocation`).
/// On Windows, PowerShell/cmd get a private temp script; on Linux the
/// command is passed straight to bash.
pub fn create_invocation(command: &str, shell: &str) -> AgentResult<ShellInvocation> {
    let (file, args) = resolve_shell(shell, "auto", command)?;
    #[cfg(windows)]
    {
        let file_name = PathBuf::from(&file)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if file_name == "powershell.exe" {
            let script = "[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)\n[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n$OutputEncoding = [Console]::OutputEncoding\n$ErrorActionPreference = 'Stop'\n$global:LASTEXITCODE = 0\ntry {\n".to_string()
                + command
                + "\nif ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }\nif (-not $?) { exit 1 }\n} catch { [Console]::Error.WriteLine($_.ToString()); exit 1 }";
            let dir = std::env::temp_dir().join(format!("muse-command-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("command.ps1");
            // Windows PowerShell 5 needs a BOM to recognize UTF-8 scripts.
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend_from_slice(script.as_bytes());
            std::fs::write(&path, bytes)?;
            return Ok(ShellInvocation {
                file,
                args: vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-File".to_string(),
                    path.to_string_lossy().to_string(),
                ],
                temp_dir: Some(dir),
            });
        }
        if file_name == "cmd.exe" {
            let dir = std::env::temp_dir().join(format!("muse-command-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("command.cmd");
            let script = "@echo off\r\nchcp 65001 >nul\r\n".to_string()
                + &command.replace("\r\n", "\n").replace('\n', "\r\n")
                + "\r\n";
            std::fs::write(&path, script)?;
            return Ok(ShellInvocation {
                file,
                args: vec![
                    "/d".to_string(),
                    "/c".to_string(),
                    "call".to_string(),
                    path.to_string_lossy().to_string(),
                ],
                temp_dir: Some(dir),
            });
        }
    }
    Ok(ShellInvocation {
        file,
        args,
        temp_dir: None,
    })
}
