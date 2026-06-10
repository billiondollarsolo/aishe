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
    /// Wall-clock duration of the last delegated command (`None` for builtins).
    last_duration: Option<Duration>,
    /// zsh `AUTO_PUSHD`: push the previous dir onto the stack on every `cd`.
    auto_pushd: bool,
    /// Extra base directories searched by `cd <name>` (zsh `cdpath`).
    cdpath: Vec<PathBuf>,
    /// Named directories for `~name` expansion (zsh hashed dirs).
    named_dirs: HashMap<String, PathBuf>,
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
            last_duration: None,
            auto_pushd: false,
            cdpath: Vec::new(),
            named_dirs: HashMap::new(),
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
    /// Wall-clock time of the last delegated command (`None` for builtins, which
    /// are instant). Used by the prompt's command-duration segment.
    pub fn last_duration(&self) -> Option<Duration> {
        self.last_duration
    }
    /// Enable zsh-style `AUTO_PUSHD`: every `cd` pushes the previous directory
    /// onto the directory stack.
    pub fn set_auto_pushd(&mut self, on: bool) {
        self.auto_pushd = on;
    }

    /// Set the `cdpath`: extra base directories searched by `cd <name>`.
    pub fn set_cdpath(&mut self, dirs: Vec<PathBuf>) {
        self.cdpath = dirs;
    }

    /// Set named directories for `~name` expansion (zsh hashed dirs).
    pub fn set_named_dirs(&mut self, dirs: HashMap<String, PathBuf>) {
        self.named_dirs = dirs;
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
        let start = Instant::now();
        let status = cmd.status();
        self.last_duration = Some(start.elapsed());

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
        // Builtins are effectively instant; don't carry a stale command duration.
        self.last_duration = None;
        let code = match tokens[0].as_str() {
            "cd" => self.builtin_cd(tokens.get(1).map(|s| s.as_str())),
            "export" => self.builtin_export(&tokens[1..]),
            "unset" => self.builtin_unset(&tokens[1..]),
            "source" | "." => self.builtin_source(tokens.get(1).map(|s| s.as_str())),
            "pushd" => self.builtin_pushd(tokens.get(1).map(|s| s.as_str())),
            "popd" => self.builtin_popd(),
            "dirs" => self.builtin_dirs(&tokens[1..]),
            other => {
                eprintln!("aishe: builtin not handled: {other}");
                1
            }
        };
        self.record(&tokens.join(" "), code);
        code
    }

    /// Resolve a `cd`/`pushd` argument to a canonical existing directory. A bare
    /// name (no `/`, `~`, or `.` prefix) that isn't found under the cwd is also
    /// searched for under each `cdpath` base directory.
    fn resolve_dir(&self, arg: &str) -> Result<PathBuf, String> {
        let target = if arg.is_empty() || arg == "~" {
            self.home()
        } else {
            self.expand_tilde(arg)
        };
        let local = if target.is_absolute() {
            target.clone()
        } else {
            self.cwd.join(&target)
        };
        // Prefer a directory under the cwd (or an absolute path).
        if let Ok(c) = local.canonicalize() {
            if c.is_dir() {
                return Ok(c);
            }
        }
        // Then try the cdpath for a bare name.
        if is_bare_name(arg) {
            for base in &self.cdpath {
                if let Ok(c) = base.join(&target).canonicalize() {
                    if c.is_dir() {
                        return Ok(c);
                    }
                }
            }
        }
        match local.canonicalize() {
            Ok(c) if c.is_dir() => Ok(c),
            Ok(_) => Err(format!("not a directory: {}", local.display())),
            Err(e) => Err(format!("{}: {e}", local.display())),
        }
    }

    /// Whether a `cd` argument resolved via the `cdpath` rather than the cwd (so
    /// `cd` should print the destination, as zsh does).
    fn resolved_via_cdpath(&self, arg: &str) -> bool {
        is_bare_name(arg) && !self.cwd.join(arg).is_dir()
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
                    self.maybe_pushd();
                    self.set_cwd(p);
                    0
                }
                None => {
                    eprintln!("cd: no previous directory");
                    1
                }
            };
        }
        // `cd -N` / `cd +N`: jump to entry N in the `dirs -v` numbering.
        if let Some(n) = arg.and_then(stack_index) {
            return self.cd_stack_index(n);
        }
        let a = arg.unwrap_or("");
        match self.resolve_dir(a) {
            Ok(dir) => {
                if dir != self.cwd {
                    self.maybe_pushd();
                }
                // zsh prints the destination when `cd` resolved via the cdpath.
                if !self.cdpath.is_empty() && self.resolved_via_cdpath(a) {
                    println!("{}", dir.display());
                }
                self.set_cwd(dir);
                0
            }
            Err(e) => {
                eprintln!("cd: {e}");
                1
            }
        }
    }

    /// Push the current dir onto the stack when `AUTO_PUSHD` is on, skipping a
    /// duplicate of the current top and capping the stack.
    fn maybe_pushd(&mut self) {
        if !self.auto_pushd {
            return;
        }
        if self.dir_stack.first() == Some(&self.cwd) {
            return;
        }
        self.dir_stack.insert(0, self.cwd.clone());
        self.dir_stack.truncate(32);
    }

    /// `cd -N` / `cd +N`: move to the Nth entry of the `dirs -v` listing (0 is the
    /// cwd; 1.. index into the stack), rotating the popped dir to the front.
    fn cd_stack_index(&mut self, n: usize) -> i32 {
        if n == 0 {
            println!("{}", self.cwd.display());
            return 0;
        }
        if n > self.dir_stack.len() {
            eprintln!("cd: no such entry in directory stack: {n}");
            return 1;
        }
        let target = self.dir_stack.remove(n - 1);
        self.dir_stack.insert(0, self.cwd.clone());
        println!("{}", target.display());
        self.set_cwd(target);
        0
    }

    /// `pushd [dir]`: with a dir, push the cwd and cd into it; with no arg, swap
    /// the cwd with the top of the stack. Prints the stack afterwards.
    fn builtin_pushd(&mut self, arg: Option<&str>) -> i32 {
        match arg {
            Some(dir) if !dir.is_empty() => match self.resolve_dir(dir) {
                Ok(target) => {
                    self.dir_stack.insert(0, self.cwd.clone());
                    self.set_cwd(target);
                    self.builtin_dirs(&[])
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
                self.builtin_dirs(&[])
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
        self.builtin_dirs(&[])
    }

    /// `dirs [-v]`: print the directory stack (cwd first), `~`-abbreviated. With
    /// `-v`, print one numbered entry per line (`cd -N` / `cd +N` jump there).
    fn builtin_dirs(&mut self, args: &[String]) -> i32 {
        let home = self.home();
        let entries: Vec<String> = std::iter::once(&self.cwd)
            .chain(self.dir_stack.iter())
            .map(|p| abbreviate_home(p, &home))
            .collect();
        if args.iter().any(|a| a == "-v") {
            for (i, e) in entries.iter().enumerate() {
                println!("{i}\t{e}");
            }
        } else {
            println!("{}", entries.join(" "));
        }
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
            return self.home();
        }
        if let Some(rest) = p.strip_prefix("~/") {
            return self.home().join(rest);
        }
        // Named directory: `~name` or `~name/sub` (zsh hashed dirs).
        if let Some(rest) = p.strip_prefix('~') {
            let (name, sub) = match rest.split_once('/') {
                Some((n, s)) => (n, Some(s)),
                None => (rest, None),
            };
            if let Some(base) = self.named_dirs.get(name) {
                return match sub {
                    Some(s) => base.join(s),
                    None => base.clone(),
                };
            }
        }
        PathBuf::from(p)
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

/// A `cd` argument that is a plain name (no `/`, `~`, or `.` prefix), and so is
/// eligible for `cdpath` lookup.
fn is_bare_name(arg: &str) -> bool {
    !arg.is_empty() && !arg.starts_with('/') && !arg.starts_with('~') && !arg.starts_with('.')
}

/// Parse a `cd -N` / `cd +N` argument into a directory-stack index. Returns
/// `None` for a bare `-`/`+` or a non-numeric argument.
fn stack_index(arg: &str) -> Option<usize> {
    let digits = arg.strip_prefix('-').or_else(|| arg.strip_prefix('+'))?;
    if digits.is_empty() {
        return None;
    }
    digits.parse::<usize>().ok()
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
    fn stack_index_parses() {
        assert_eq!(stack_index("-2"), Some(2));
        assert_eq!(stack_index("+3"), Some(3));
        assert_eq!(stack_index("-"), None);
        assert_eq!(stack_index("foo"), None);
        assert_eq!(stack_index("-x"), None);
    }

    #[test]
    fn auto_pushd_and_cd_stack_navigation() {
        let mut ex = Executor::new().unwrap();
        ex.set_auto_pushd(true);
        let start = ex.cwd().canonicalize().unwrap();
        let tmp = std::env::temp_dir().canonicalize().unwrap();

        // AUTO_PUSHD: cd into tmp pushes the start dir onto the stack.
        assert_eq!(ex.run_builtin(&["cd".into(), tmp.display().to_string()]), 0);
        assert_eq!(ex.cwd().canonicalize().unwrap(), tmp);
        assert_eq!(ex.dir_stack.len(), 1);

        // `cd -1` jumps back to the start dir (stack entry 1).
        assert_eq!(ex.run_builtin(&["cd".into(), "-1".into()]), 0);
        assert_eq!(ex.cwd().canonicalize().unwrap(), start);

        // out-of-range index is an error.
        assert_eq!(ex.run_builtin(&["cd".into(), "-9".into()]), 1);
    }

    #[test]
    fn is_bare_name_works() {
        assert!(is_bare_name("projects"));
        assert!(!is_bare_name("/abs"));
        assert!(!is_bare_name("~/x"));
        assert!(!is_bare_name("./rel"));
        assert!(!is_bare_name("../up"));
        assert!(!is_bare_name(""));
    }

    #[test]
    fn cdpath_resolves_bare_name() {
        let base = std::env::temp_dir().join(format!("aishe-cdp-{}", std::process::id()));
        // A name unlikely to exist under the test's cwd, so cdpath is consulted.
        let name = "cdp_target_xyz";
        let target = base.join(name);
        std::fs::create_dir_all(&target).unwrap();

        let mut ex = Executor::new().unwrap();
        ex.set_cdpath(vec![base.clone()]);
        // `cd <name>` is not under the cwd, so it resolves via the cdpath.
        assert_eq!(ex.run_builtin(&["cd".into(), name.into()]), 0);
        assert_eq!(
            ex.cwd().canonicalize().unwrap(),
            target.canonicalize().unwrap()
        );
        // a name that exists in neither cwd nor cdpath still errors.
        assert_eq!(ex.run_builtin(&["cd".into(), "no_such_dir_xyz".into()]), 1);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn named_dir_expansion() {
        let base = std::env::temp_dir()
            .join(format!("aishe-named-{}", std::process::id()))
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        let mut ex = Executor::new().unwrap();
        ex.set_named_dirs(HashMap::from([("proj".to_string(), base.clone())]));

        // `cd ~proj` lands in the named base directory.
        assert_eq!(ex.run_builtin(&["cd".into(), "~proj".into()]), 0);
        assert_eq!(
            ex.cwd().canonicalize().unwrap(),
            base.canonicalize().unwrap()
        );

        // `cd ~proj/sub` joins the subpath.
        assert_eq!(ex.run_builtin(&["cd".into(), "~proj/sub".into()]), 0);
        assert_eq!(
            ex.cwd().canonicalize().unwrap(),
            sub.canonicalize().unwrap()
        );

        // an unknown name is left literal (and fails to resolve).
        assert_eq!(ex.run_builtin(&["cd".into(), "~nope/x".into()]), 1);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn dirs_verbose_numbers_entries() {
        let mut ex = Executor::new().unwrap();
        ex.set_auto_pushd(true);
        let tmp = std::env::temp_dir().display().to_string();
        ex.run_builtin(&["cd".into(), tmp]);
        // `dirs -v` returns 0; the numbered output goes to stdout.
        assert_eq!(ex.run_builtin(&["dirs".into(), "-v".into()]), 0);
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
