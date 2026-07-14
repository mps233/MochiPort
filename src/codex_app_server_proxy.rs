use std::{
    collections::HashSet,
    ffi::OsString,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

pub const CODEX_CLI_PATH_ENV: &str = "CODEX_CLI_PATH";
pub const REAL_CODEX_CLI_PATH_ENV: &str = "CODEXHUB_REAL_CODEX_CLI_PATH";

pub fn is_proxy_invocation() -> bool {
    let has_app_server_arg = std::env::args_os().skip(1).any(|arg| arg == "app-server");
    if !has_app_server_arg {
        return false;
    }

    let Some(configured_proxy) = std::env::var_os(CODEX_CLI_PATH_ENV) else {
        return false;
    };
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    paths_equivalent(Path::new(&configured_proxy), &current_exe)
}

pub fn run() -> Result<i32> {
    let current_exe = std::env::current_exe().context("failed to locate CodexHub executable")?;
    let real_codex = resolve_real_codex_cli(&current_exe)?;
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut child = spawn_real_codex(&real_codex, &args)?;
    proxy_stdio(&mut child)
}

fn spawn_real_codex(path: &Path, args: &[OsString]) -> Result<Child> {
    let mut command = Command::new(path);
    command
        .args(args)
        .env_remove(CODEX_CLI_PATH_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .with_context(|| format!("failed to start real Codex CLI {}", path.display()))
}

fn proxy_stdio(child: &mut Child) -> Result<i32> {
    let child_stdin = child
        .stdin
        .take()
        .context("real Codex stdin is unavailable")?;
    let child_stdout = child
        .stdout
        .take()
        .context("real Codex stdout is unavailable")?;
    let child_stderr = child
        .stderr
        .take()
        .context("real Codex stderr is unavailable")?;
    let pending_account_reads = Arc::new(Mutex::new(HashSet::<String>::new()));

    let request_ids = Arc::clone(&pending_account_reads);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut writer = child_stdin;
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Some(id) = account_read_request_id(&line) {
                        request_ids
                            .lock()
                            .expect("request ID lock poisoned")
                            .insert(id);
                    }
                    if writer.write_all(line.as_bytes()).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let stderr_thread = std::thread::spawn(move || {
        let mut reader = child_stderr;
        let mut stderr = io::stderr().lock();
        let _ = io::copy(&mut reader, &mut stderr);
        let _ = stderr.flush();
    });

    let mut reader = BufReader::new(child_stdout);
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let rewritten = rewrite_account_read_response(&line, &pending_account_reads);
        writer.write_all(rewritten.as_bytes())?;
        writer.flush()?;
    }

    let status = child.wait().context("failed to wait for real Codex CLI")?;
    let _ = stderr_thread.join();
    Ok(exit_code(status))
}

fn account_read_request_id(line: &str) -> Option<String> {
    let request: Value = serde_json::from_str(line.trim()).ok()?;
    (request.get("method").and_then(Value::as_str) == Some("account/read"))
        .then(|| request.get("id").and_then(request_id_key))
        .flatten()
}

fn rewrite_account_read_response(
    line: &str,
    pending_account_reads: &Mutex<HashSet<String>>,
) -> String {
    let Ok(mut response) = serde_json::from_str::<Value>(line.trim()) else {
        return line.to_string();
    };
    let Some(id) = response.get("id").and_then(request_id_key) else {
        return line.to_string();
    };
    if !pending_account_reads
        .lock()
        .expect("request ID lock poisoned")
        .remove(&id)
    {
        return line.to_string();
    }

    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else {
        return line.to_string();
    };
    result.insert(
        "account".to_string(),
        json!({
            "type": "chatgpt",
            "email": "codexhub-local@example.local",
            "planType": "pro",
        }),
    );
    result.insert("requiresOpenaiAuth".to_string(), Value::Bool(false));

    let mut rewritten = response.to_string();
    if line.ends_with("\r\n") {
        rewritten.push_str("\r\n");
    } else if line.ends_with('\n') {
        rewritten.push('\n');
    }
    rewritten
}

fn request_id_key(id: &Value) -> Option<String> {
    matches!(id, Value::String(_) | Value::Number(_)).then(|| id.to_string())
}

pub(crate) fn resolve_real_codex_cli(current_exe: &Path) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(REAL_CODEX_CLI_PATH_ENV).map(PathBuf::from)
        && is_usable_real_cli(&path, current_exe)
    {
        return Ok(path);
    }

    if let Some(path) = resolve_platform_codex_cli(current_exe) {
        return Ok(path);
    }

    Err(anyhow!(
        "unable to locate the real Codex CLI; reinstall or reinitialize CodexHub"
    ))
}

pub(crate) fn resolve_platform_codex_cli(current_exe: &Path) -> Option<PathBuf> {
    platform_codex_cli_candidates()
        .into_iter()
        .find(|path| is_usable_real_cli(path, current_exe))
}

fn is_usable_real_cli(path: &Path, current_exe: &Path) -> bool {
    path.is_file() && !paths_equivalent(path, current_exe) && !is_windows_store_package_path(path)
}

fn is_windows_store_package_path(path: &Path) -> bool {
    cfg!(target_os = "windows")
        && path
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase()
            .contains("\\program files\\windowsapps\\")
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    if cfg!(target_os = "windows") {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

#[cfg(target_os = "windows")]
fn platform_codex_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("bin");
        let mut cached = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("codex.exe"))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        cached.sort_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        cached.reverse();
        candidates.extend(cached);
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let codex_home = PathBuf::from(home).join(".codex");
        candidates.push(
            codex_home
                .join("plugins")
                .join(".plugin-appserver")
                .join("codex.exe"),
        );
        candidates.push(codex_home.join(".sandbox-bin").join("codex.exe"));
    }
    candidates.extend(command_path_candidates("where.exe", "codex.exe"));
    candidates
}

#[cfg(not(target_os = "windows"))]
fn platform_codex_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = command_path_candidates("which", "codex");
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(
            "/Applications/Codex.app/Contents/Resources/codex",
        ));
    }
    candidates
}

fn command_path_candidates(command: &str, executable: &str) -> Vec<PathBuf> {
    let Ok(output) = Command::new(command).arg(executable).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_matching_account_read_response() {
        let pending = Mutex::new(HashSet::from(["7".to_string()]));
        let line = "{\"id\":7,\"result\":{\"account\":null,\"requiresOpenaiAuth\":false}}\n";
        let rewritten = rewrite_account_read_response(line, &pending);
        let value: Value = serde_json::from_str(rewritten.trim()).expect("valid JSON");

        assert_eq!(value["result"]["account"]["type"], "chatgpt");
        assert_eq!(value["result"]["account"]["planType"], "pro");
        assert_eq!(value["result"]["requiresOpenaiAuth"], false);
        assert!(pending.lock().expect("lock").is_empty());
    }

    #[test]
    fn leaves_unrelated_messages_byte_for_byte_unchanged() {
        let pending = Mutex::new(HashSet::from(["7".to_string()]));
        let line = "{ \"id\": 8, \"result\": { \"models\": [] } }\r\n";

        assert_eq!(rewrite_account_read_response(line, &pending), line);
        assert!(pending.lock().expect("lock").contains("7"));
    }

    #[test]
    fn recognizes_string_and_numeric_account_read_ids() {
        assert_eq!(
            account_read_request_id(r#"{"method":"account/read","id":"auth-1","params":{}}"#),
            Some("\"auth-1\"".to_string())
        );
        assert_eq!(
            account_read_request_id(r#"{"method":"account/read","id":42,"params":{}}"#),
            Some("42".to_string())
        );
        assert_eq!(
            account_read_request_id(r#"{"method":"model/list","id":42,"params":{}}"#),
            None
        );
    }
}
