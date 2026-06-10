//! Command execution. We do not reimplement a shell grammar: shell lines are
//! delegated to `zsh -c` (fallback `bash -c`). A handful of builtins that must
//! mutate persistent shell state (`cd`, `export`, …) are intercepted in-process.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
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
    /// Temp rc file sourced into every delegated command: the user's `.aishrc`
    /// files plus interactively-defined aliases/options replayed for persistence.
    /// `None` if it couldn't be created (commands then run without it).
    session_rc: Option<PathBuf>,
    /// Directory stack for `pushd`/`popd`/`dirs` (most-recent first; the cwd is
    /// not stored here — it's shown first by `dirs`).
    dir_stack: Vec<PathBuf>,
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
            session_rc: init_session_rc().ok(),
            dir_stack: Vec::new(),
        })
    }

    pub fn shell(&self) -> &PathBuf {
        &self.shell
    }
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// Configure a `zsh -c`/`bash -c` invocation to source the session rc (user
    /// `.aishrc` + replayed definitions) before running `line`.
    ///
    /// The command is passed via `$AISHE_CMD` and run through `eval` *after* the
    /// rc is sourced. This matters because aliases are resolved at parse time:
    /// `source rc; greet` would parse `greet` before the alias exists, whereas
    /// `eval "$AISHE_CMD"` re-parses at runtime once the alias is defined.
    fn apply_rc(&self, cmd: &mut Command, line: &str) {
        cmd.arg("-c");
        match &self.session_rc {
            Some(rc) => {
                cmd.arg(format!(
                    "source {} 2>/dev/null; eval \"$AISHE_CMD\"",
                    single_quote(rc)
                ));
                cmd.env("AISHE_CMD", line);
            }
            None => {
                cmd.arg(line);
            }
        }
    }

    /// Append an interactively-typed alias/option definition to the session rc
    /// so it persists into later commands (the reedline front-end runs each line
    /// in a fresh shell, so state would otherwise be lost).
    fn persist_definition(&mut self, line: &str) {
        let (Some(rc), Some(def)) = (&self.session_rc, persistable_definition(line)) else {
            return;
        };
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(rc) {
            let _ = writeln!(f, "{def}");
        }
    }

    /// Append a whole function definition to the session rc so it persists into
    /// later commands (re-defining a function on each `source` is cheap and has
    /// no side effects — the body only runs when the function is called).
    fn persist_function(&mut self, line: &str) {
        let Some(rc) = &self.session_rc else { return };
        if crate::dispatcher::function_def_name(line).is_some() {
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(rc) {
                let _ = writeln!(f, "{line}");
            }
        }
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
        let mut cmd = Command::new(&self.shell);
        self.apply_rc(&mut cmd, line);
        cmd.envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = cmd.status();

        let code = match status {
            Ok(s) => exit_code(&s),
            Err(e) => {
                eprintln!("aishe: failed to launch shell: {e}");
                127
            }
        };
        if code == 0 {
            self.persist_definition(line);
            self.persist_function(line);
        }
        self.record(line, code);
        code
    }

    /// Run a command capturing merged stdout+stderr (tee'd to the terminal),
    /// with a timeout and stdin closed. Returns (exit_code, truncated_output).
    pub fn run_captured(&mut self, line: &str, timeout: Duration) -> (i32, String) {
        let mut cmd = Command::new(&self.shell);
        self.apply_rc(&mut cmd, line);
        let child = cmd
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
            "pushd" => self.builtin_pushd(tokens.get(1).map(|s| s.as_str())),
            "popd" => self.builtin_popd(),
            "dirs" => self.builtin_dirs(),
            other => {
                eprintln!("aishe: builtin not handled: {other}");
                1
            }
        };
        self.record(&tokens.join(" "), code);
        code
    }

    /// Resolve a `cd`/`pushd` argument to a canonical existing directory.
    fn resolve_dir(&self, arg: &str) -> Result<PathBuf, String> {
        let target = if arg.is_empty() || arg == "~" {
            self.home()
        } else {
            self.expand_tilde(arg)
        };
        let target = if target.is_absolute() {
            target
        } else {
            self.cwd.join(target)
        };
        match target.canonicalize() {
            Ok(c) if c.is_dir() => Ok(c),
            Ok(_) => Err(format!("not a directory: {}", target.display())),
            Err(e) => Err(format!("{}: {e}", target.display())),
        }
    }

    /// Move to `new`, recording the previous dir and updating `$PWD`.
    fn set_cwd(&mut self, new: PathBuf) {
        self.env
            .insert("PWD".to_string(), new.display().to_string());
        self.prev_cwd = Some(std::mem::replace(&mut self.cwd, new));
    }

    fn builtin_cd(&mut self, arg: Option<&str>) -> i32 {
        if arg == Some("-") {
            return match self.prev_cwd.clone() {
                Some(p) => {
                    println!("{}", p.display());
                    self.set_cwd(p);
                    0
                }
                None => {
                    eprintln!("cd: no previous directory");
                    1
                }
            };
        }
        match self.resolve_dir(arg.unwrap_or("")) {
            Ok(dir) => {
                self.set_cwd(dir);
                0
            }
            Err(e) => {
                eprintln!("cd: {e}");
                1
            }
        }
    }

    /// `pushd [dir]`: with a dir, push the cwd and cd into it; with no arg, swap
    /// the cwd with the top of the stack. Prints the stack afterwards.
    fn builtin_pushd(&mut self, arg: Option<&str>) -> i32 {
        match arg {
            Some(dir) if !dir.is_empty() => match self.resolve_dir(dir) {
                Ok(target) => {
                    self.dir_stack.insert(0, self.cwd.clone());
                    self.set_cwd(target);
                    self.builtin_dirs()
                }
                Err(e) => {
                    eprintln!("pushd: {e}");
                    1
                }
            },
            _ => {
                if self.dir_stack.is_empty() {
                    eprintln!("pushd: no other directory");
                    return 1;
                }
                let top = self.dir_stack.remove(0);
                let old = std::mem::replace(&mut self.cwd, top);
                self.dir_stack.insert(0, old);
                self.env
                    .insert("PWD".to_string(), self.cwd.display().to_string());
                self.builtin_dirs()
            }
        }
    }

    /// `popd`: pop the top of the stack and cd into it.
    fn builtin_popd(&mut self) -> i32 {
        if self.dir_stack.is_empty() {
            eprintln!("popd: directory stack empty");
            return 1;
        }
        let top = self.dir_stack.remove(0);
        self.set_cwd(top);
        self.builtin_dirs()
    }

    /// `dirs`: print the directory stack (cwd first), `~`-abbreviated.
    fn builtin_dirs(&mut self) -> i32 {
        let mut entries = vec![abbreviate_home(&self.cwd, &self.home())];
        entries.extend(
            self.dir_stack
                .iter()
                .map(|p| abbreviate_home(p, &self.home())),
        );
        println!("{}", entries.join(" "));
        0
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

/// Create the per-session rc file that every delegated command sources. It
/// begins by loading the user's `~/.aishrc` and `~/.config/aishe/aishrc` (if
/// present); interactively-defined aliases/options are appended later.
fn init_session_rc() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("aishe-session-{}.zsh", std::process::id()));
    // Ensure aliases expand in the non-interactive `-c` shell (bash needs the
    // shopt; zsh has it on by default). Each line is harmless in the other shell.
    let mut content = String::from(
        "# aishe session rc (generated)\n\
         shopt -s expand_aliases 2>/dev/null\n\
         setopt aliases 2>/dev/null\n",
    );
    if let Some(home) = dirs::home_dir() {
        let p = single_quote(&home.join(".aishrc"));
        content.push_str(&format!("[ -f {p} ] && source {p}\n"));
    }
    if let Some(cfg) = dirs::config_dir() {
        let p = single_quote(&cfg.join("aishe").join("aishrc"));
        content.push_str(&format!("[ -f {p} ] && source {p}\n"));
    }
    std::fs::write(&path, content)?;
    Ok(path)
}

/// Render a path with a leading `$HOME` abbreviated to `~` (for `dirs`).
fn abbreviate_home(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Single-quote a path for safe interpolation into a shell command.
fn single_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// If `line` is *solely* an alias/option definition, return the text to replay
/// in later commands; otherwise `None`. Lines with shell operators (`;`, `|`,
/// `&`, newlines) are rejected so we never replay a whole pipeline.
fn persistable_definition(line: &str) -> Option<String> {
    let t = line.trim();
    if t.contains(';') || t.contains('|') || t.contains('&') || t.contains('\n') {
        return None;
    }
    let (cmd, rest) = match t.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (t, ""),
    };
    match cmd {
        // `alias x=y` defines; bare `alias` just lists — don't replay the latter.
        "alias" if rest.contains('=') => Some(t.to_string()),
        "unalias" | "setopt" | "unsetopt" if !rest.is_empty() => Some(t.to_string()),
        _ => None,
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        if let Some(rc) = &self.session_rc {
            let _ = std::fs::remove_file(rc);
        }
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

    #[test]
    fn persistable_definitions_detected() {
        assert_eq!(
            persistable_definition("alias g=git"),
            Some("alias g=git".to_string())
        );
        assert_eq!(
            persistable_definition("  setopt extended_glob "),
            Some("setopt extended_glob".to_string())
        );
        assert_eq!(
            persistable_definition("unalias g"),
            Some("unalias g".to_string())
        );
    }

    #[test]
    fn non_definitions_not_persisted() {
        // bare listing forms, non-definitions, and anything with operators.
        assert_eq!(persistable_definition("alias"), None);
        assert_eq!(persistable_definition("setopt"), None);
        assert_eq!(persistable_definition("ls -la"), None);
        assert_eq!(persistable_definition("alias g=git; rm -rf build"), None);
        assert_eq!(persistable_definition("alias g=git && echo hi"), None);
    }

    #[test]
    fn dir_stack_pushd_popd() {
        let mut ex = Executor::new().unwrap();
        let start = ex.cwd().clone();
        let tmp = std::env::temp_dir();

        assert_eq!(
            ex.run_builtin(&["pushd".into(), tmp.display().to_string()]),
            0
        );
        assert_eq!(
            ex.cwd().canonicalize().unwrap(),
            tmp.canonicalize().unwrap()
        );

        assert_eq!(ex.run_builtin(&["popd".into()]), 0);
        assert_eq!(ex.cwd(), &start);

        // popd on an empty stack is an error.
        assert_eq!(ex.run_builtin(&["popd".into()]), 1);
    }

    #[test]
    fn abbreviate_home_works() {
        let home = Path::new("/home/u");
        assert_eq!(abbreviate_home(Path::new("/home/u"), home), "~");
        assert_eq!(abbreviate_home(Path::new("/home/u/p"), home), "~/p");
        assert_eq!(abbreviate_home(Path::new("/etc"), home), "/etc");
    }

    #[test]
    fn single_quote_escapes() {
        assert_eq!(single_quote(std::path::Path::new("/tmp/x")), "'/tmp/x'");
        assert_eq!(single_quote(std::path::Path::new("/a'b")), r"'/a'\''b'");
    }
}
