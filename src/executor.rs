//! Command execution. We do not reimplement a shell grammar: shell lines are
//! delegated to `zsh -c` (fallback `bash -c`). A handful of builtins that must
//! mutate persistent shell state (`cd`, `export`, …) are intercepted in-process.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

/// Captured-output truncation limit handed to the LLM.
const CAPTURE_TRUNCATE_CHARS: usize = 8_000;
/// Default timeout for captured (yolo) commands.
pub const DEFAULT_CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Executor {
    /// Backing shell: zsh if available, else bash.
    shell: PathBuf,
    /// Mutable environment snapshot applied to every child.
    env: HashMap<String, String>,
    cwd: PathBuf,
    prev_cwd: Option<PathBuf>,
    pub last_exit: i32,
    /// Last 10 (command, exit_code) pairs, for LLM context.
    pub history: VecDeque<(String, i32)>,
}

impl Executor {
    /// Construct an executor, locating a backing shell. Errors if neither zsh
    /// nor bash is on `$PATH`.
    pub fn new() -> Result<Self> {
        let shell = which("zsh")
            .or_else(|| which("bash"))
            .ok_or_else(|| anyhow!("neither zsh nor bash found on $PATH"))?;
        let env: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Ok(Self {
            shell,
            env,
            cwd,
            prev_cwd: None,
            last_exit: 0,
            history: VecDeque::with_capacity(10),
        })
    }

    pub fn shell(&self) -> &PathBuf {
        &self.shell
    }
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    fn record(&mut self, line: &str, code: i32) {
        self.last_exit = code;
        if self.history.len() == 10 {
            self.history.pop_front();
        }
        self.history.push_back((line.to_string(), code));
    }

    /// Delegate a shell line to the backing shell with inherited stdio, so
    /// interactive children (vim, ssh, top) and pipes/globs/redirs all work.
    pub fn run(&mut self, line: &str) -> i32 {
        let status = Command::new(&self.shell)
            .arg("-c")
            .arg(line)
            .envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();

        let code = match status {
            Ok(s) => exit_code(&s),
            Err(e) => {
                eprintln!("aishe: failed to launch shell: {e}");
                127
            }
        };
        self.record(line, code);
        code
    }

    /// Run a command capturing merged stdout+stderr (tee'd to the terminal),
    /// with a timeout and stdin closed. Returns (exit_code, truncated_output).
    pub fn run_captured(&mut self, line: &str, timeout: Duration) -> (i32, String) {
        let child = Command::new(&self.shell)
            .arg("-c")
            .arg(line)
            .envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("aishe: failed to launch shell: {e}");
                eprintln!("{msg}");
                self.record(line, 127);
                return (127, msg);
            }
        };

        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut drainers = Vec::new();

        if let Some(out) = child.stdout.take() {
            drainers.push(spawn_drainer(out, Arc::clone(&collected), false));
        }
        if let Some(err) = child.stderr.take() {
            drainers.push(spawn_drainer(err, Arc::clone(&collected), true));
        }

        // Poll for completion, enforcing the timeout.
        let start = Instant::now();
        let mut timed_out = false;
        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break exit_code(&status),
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break 137; // 128 + SIGKILL(9)
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break 1,
            }
        };

        for d in drainers {
            let _ = d.join();
        }

        let mut output = collected.lock().unwrap().join("\n");
        if timed_out {
            let note = format!(
                "\n[aishe: command timed out after {}s and was killed]",
                timeout.as_secs()
            );
            output.push_str(&note);
        }
        let output = truncate_tail(&output, CAPTURE_TRUNCATE_CHARS);
        self.record(line, code);
        (code, output)
    }

    /// Handle an intercepted builtin. Returns the resulting exit code.
    pub fn run_builtin(&mut self, tokens: &[String]) -> i32 {
        let code = match tokens[0].as_str() {
            "cd" => self.builtin_cd(tokens.get(1).map(|s| s.as_str())),
            "export" => self.builtin_export(&tokens[1..]),
            "unset" => self.builtin_unset(&tokens[1..]),
            "source" | "." => self.builtin_source(tokens.get(1).map(|s| s.as_str())),
            other => {
                eprintln!("aishe: builtin not handled: {other}");
                1
            }
        };
        self.record(&tokens.join(" "), code);
        code
    }

    fn builtin_cd(&mut self, arg: Option<&str>) -> i32 {
        let target: PathBuf = match arg {
            None | Some("") | Some("~") => self.home(),
            Some("-") => match &self.prev_cwd {
                Some(p) => {
                    println!("{}", p.display());
                    p.clone()
                }
                None => {
                    eprintln!("cd: no previous directory");
                    return 1;
                }
            },
            Some(p) => self.expand_tilde(p),
        };
        let target = if target.is_absolute() {
            target
        } else {
            self.cwd.join(target)
        };
        match target.canonicalize() {
            Ok(canonical) if canonical.is_dir() => {
                self.prev_cwd = Some(std::mem::replace(&mut self.cwd, canonical.clone()));
                // Keep PWD in sync for child processes.
                self.env
                    .insert("PWD".to_string(), canonical.display().to_string());
                0
            }
            Ok(_) => {
                eprintln!("cd: not a directory: {}", target.display());
                1
            }
            Err(e) => {
                eprintln!("cd: {}: {e}", target.display());
                1
            }
        }
    }

    fn builtin_export(&mut self, args: &[String]) -> i32 {
        if args.is_empty() {
            // Mimic `export` with no args: list exported vars.
            let mut keys: Vec<_> = self.env.keys().collect();
            keys.sort();
            for k in keys {
                println!("export {}={}", k, self.env[k]);
            }
            return 0;
        }
        for arg in args {
            if let Some((k, v)) = arg.split_once('=') {
                let v = strip_quotes(v);
                self.env.insert(k.to_string(), v);
            } else {
                // `export K` promotes an existing process env var if present.
                if let Ok(v) = std::env::var(arg) {
                    self.env.insert(arg.clone(), v);
                }
            }
        }
        0
    }

    fn builtin_unset(&mut self, args: &[String]) -> i32 {
        for k in args {
            self.env.remove(k);
        }
        0
    }

    /// Source a file by running it in the backing shell and diffing the env.
    /// Note: aliases/functions defined by the file do NOT persist.
    fn builtin_source(&mut self, file: Option<&str>) -> i32 {
        let file = match file {
            Some(f) if !f.is_empty() => f,
            _ => {
                eprintln!("source: filename argument required");
                return 1;
            }
        };
        let script = format!("source {file} >/dev/null 2>&1 && env -0");
        let out = Command::new(&self.shell)
            .arg("-c")
            .arg(&script)
            .envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .output();

        match out {
            Ok(o) if o.status.success() => {
                for entry in o.stdout.split(|&b| b == 0) {
                    if entry.is_empty() {
                        continue;
                    }
                    if let Ok(s) = std::str::from_utf8(entry) {
                        if let Some((k, v)) = s.split_once('=') {
                            self.env.insert(k.to_string(), v.to_string());
                        }
                    }
                }
                0
            }
            Ok(o) => {
                eprintln!("source: failed to source {file}");
                exit_code(&o.status).max(1)
            }
            Err(e) => {
                eprintln!("source: {e}");
                1
            }
        }
    }

    fn home(&self) -> PathBuf {
        self.env
            .get("HOME")
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    fn expand_tilde(&self, p: &str) -> PathBuf {
        if p == "~" {
            self.home()
        } else if let Some(rest) = p.strip_prefix("~/") {
            self.home().join(rest)
        } else {
            PathBuf::from(p)
        }
    }
}

/// Spawn a thread draining a child pipe, tee'ing each line to the terminal
/// (stderr lines to stderr) and collecting it for the LLM.
fn spawn_drainer<R: std::io::Read + Send + 'static>(
    reader: R,
    collected: Arc<Mutex<Vec<String>>>,
    is_stderr: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines().map_while(Result::ok) {
            if is_stderr {
                let mut e = std::io::stderr();
                let _ = writeln!(e, "{line}");
            } else {
                let mut o = std::io::stdout();
                let _ = writeln!(o, "{line}");
            }
            collected.lock().unwrap().push(line);
        }
    })
}

/// Derive an exit code, mapping signal termination to 128 + signal.
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Keep only the last `max` characters of `s`, prefixing a truncation marker.
fn truncate_tail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let tail: String = s
        .chars()
        .rev()
        .take(max)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("[... output truncated to last {max} chars ...]\n{tail}")
}

fn strip_quotes(v: &str) -> String {
    let v = v.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// Find an executable on `$PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_tail() {
        let s: String = (0..10_000).map(|_| 'a').collect();
        let t = truncate_tail(&s, 8_000);
        assert!(t.contains("truncated"));
        // Exactly the last 8000 chars are preserved at the end.
        assert!(t.ends_with(&"a".repeat(8_000)));
        assert!(!t.ends_with(&"a".repeat(8_001)));
    }

    #[test]
    fn truncate_noop_when_short() {
        assert_eq!(truncate_tail("hello", 8_000), "hello");
    }

    #[test]
    fn strip_quotes_works() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
        assert_eq!(strip_quotes("'hi'"), "hi");
        assert_eq!(strip_quotes("plain"), "plain");
    }
}
