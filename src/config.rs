//! Configuration: TOML at `~/.config/aishe/config.toml`, with an interactive
//! first-run wizard when missing and graceful recovery when malformed.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::theme::ThemeConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub aishe: AisheConfig,
    #[serde(default)]
    pub providers: Providers,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Per-model price overrides (USD per 1M tokens) for cost estimates, keyed by
    /// model name or substring. Falls back to a built-in table; see `usage.rs`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub pricing: std::collections::BTreeMap<String, crate::usage::Price>,
    /// Named directories for `~name` expansion in `cd` (zsh hashed dirs), e.g.
    /// `proj = "/home/me/projects"`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub named_dirs: std::collections::BTreeMap<String, String>,
    /// MCP (Model Context Protocol) servers whose tools are offered to yolo,
    /// keyed by a short name used to namespace them (`mcp__<name>__<tool>`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub mcp_servers: std::collections::BTreeMap<String, McpServerConfig>,
}

/// A configured MCP server. Either a stdio server (launched via `command`) or a
/// Streamable HTTP server (reached at `url`). A server is HTTP when `url` is set;
/// otherwise it is stdio. If neither is set the server is invalid and skipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Executable to spawn for a stdio server (e.g. `npx`, `uvx`, or an absolute
    /// path). Optional: omit it for an HTTP server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments passed to the command (stdio servers only).
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the server process (stdio servers only).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// Endpoint URL for an MCP Streamable HTTP server. When set, the server is
    /// reached over HTTP instead of stdio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Extra HTTP request headers for an HTTP server (e.g. `Authorization`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Connect to this server. On by default; set `false` to keep it configured
    /// but disabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Audit logging of AI calls, responses, and AI-initiated actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Write a JSONL audit log. Off by default (it records prompts/outputs to
    /// disk). Also enableable with `AISHE_LOG=1`.
    #[serde(default)]
    pub enabled: bool,
    /// Log file path. Defaults to `$XDG_DATA_HOME/aishe/audit.jsonl`. Override
    /// with `AISHE_LOG_FILE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Redact likely secrets from logged text. On by default.
    #[serde(default = "default_true")]
    pub redact: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            file: None,
            redact: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AisheConfig {
    /// "suggest" | "yolo"
    #[serde(default = "default_mode")]
    pub mode: String,
    /// "anthropic" | "openai"
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_true")]
    pub yolo_confirm_dangerous: bool,
    /// When the yolo loop pauses to confirm a `run_command` call: "never",
    /// "dangerous" (only safety-flagged commands, the default), "writes"
    /// (dangerous plus any state-modifying command), or "all". The older
    /// `yolo_confirm_dangerous` boolean is honored for backward compatibility;
    /// see `sandbox::confirm_tier` for the precedence.
    #[serde(default = "default_yolo_confirm")]
    pub yolo_confirm: String,
    /// Policy-based sandbox for the yolo loop: when on, a `run_command` that
    /// reaches the network or writes outside the working tree is refused (fed
    /// back to the model as an error) instead of running. Best-effort, not a
    /// kernel sandbox. Off by default. Toggle with `aishe sandbox on`.
    #[serde(default)]
    pub yolo_sandbox: bool,
    #[serde(default = "default_max_iters")]
    pub max_yolo_iterations: u32,
    /// Plan-first (dry run): before the yolo loop runs anything, ask the model
    /// for its intended steps, show them, and require approval. Interactive only.
    /// Off by default.
    #[serde(default)]
    pub yolo_plan: bool,
    /// Include a per-project `.aishe/context.md` (found at or above the cwd) in
    /// the model context, so repo-specific conventions are available. On by
    /// default.
    #[serde(default = "default_true")]
    pub project_context: bool,
    /// Cache identical suggest-mode model responses for a short while to make
    /// repeats instant and free. On by default.
    #[serde(default = "default_true")]
    pub cache: bool,
    /// How long a cached response stays valid, in seconds.
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
    /// Offer the built-in file tools (`read_file`/`write_file`/`edit_file`/
    /// `list_dir`) to yolo, so it can work with files directly instead of via the
    /// shell. On by default.
    #[serde(default = "default_true")]
    pub file_tools: bool,
    /// Offer the built-in `fetch_url` tool to yolo, so it can read web pages and
    /// docs (HTTP GET, HTML stripped to text, size-capped). On by default.
    #[serde(default = "default_true")]
    pub web_tool: bool,
    #[serde(default = "default_true")]
    pub show_right_prompt: bool,
    /// Front-end: "auto" (default — zsh-pty when zsh is on $PATH, else
    /// reedline), "reedline" (built-in editor), or "zsh-pty" (drive the user's
    /// real interactive zsh in a PTY, so all native zsh plugins work).
    #[serde(default = "default_front_end")]
    pub front_end: String,
    /// In the zsh-PTY front-end, override the prompt with aishe's branded prompt
    /// (`<cwd> <glyph>`, glyph per mode) so it's obvious you're in aishe. On by
    /// default; set false to keep your real zsh prompt untouched.
    #[serde(default = "default_true")]
    pub pty_prompt: bool,
    /// reedline line-editor keymap: "emacs" (default) or "vi".
    #[serde(default = "default_edit_mode")]
    pub edit_mode: String,
    /// Optional custom left-prompt format for the reedline front-end. Supports
    /// `{cwd}`, `{mode}`, `{model}`, `{exit}`. `None` = just the cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_format: Option<String>,
    /// Show a git branch segment in the right prompt (reedline front-end).
    #[serde(default = "default_true")]
    pub git_prompt: bool,
    /// Add dirty (`*`) and ahead/behind (`⇡`/`⇣`) markers to the git segment
    /// (one short `git status` call per prompt). Disable in huge repos.
    #[serde(default = "default_true")]
    pub git_status: bool,
    /// Show the last command's duration in the right prompt when it took at least
    /// this many seconds. `0` disables it.
    #[serde(default = "default_report_time")]
    pub report_time: u64,
    /// zsh `AUTO_PUSHD`: every `cd` pushes the previous directory onto the stack
    /// (navigate with `cd -N` / `cd +N`, list with `dirs -v`).
    #[serde(default)]
    pub auto_pushd: bool,
    /// Don't save a command to history when it equals the previous one
    /// (`HIST_IGNORE_DUPS`).
    #[serde(default = "default_true")]
    pub hist_ignore_dups: bool,
    /// Don't save commands to history when it starts with a space
    /// (`HIST_IGNORE_SPACE`).
    #[serde(default)]
    pub hist_ignore_space: bool,
    /// Glob patterns (`*`/`?`) of commands to keep out of history (`HISTIGNORE`),
    /// e.g. `["ls", "cd *", "* --help"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hist_ignore: Vec<String>,
    /// Extra base directories searched by `cd <name>` (`CDPATH`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cdpath: Vec<String>,
    /// zsh `CORRECT`: when an unknown first word is a near-miss of a known
    /// command, offer to correct it instead of treating the line as natural
    /// language. Off by default.
    #[serde(default)]
    pub correct: bool,
    /// Complete a command's flags from its `--help` output when you tab-complete a
    /// word starting with `-` (parsed, cached, time-limited). On by default.
    #[serde(default = "default_true")]
    pub complete_flags: bool,
    /// Share one timestamped history across sessions (zsh `SHARE_HISTORY`): the
    /// `history` builtin and the reedline history file are shared, so commands
    /// from other sessions are visible. When off, history is per-session
    /// (pid-suffixed files). On by default.
    #[serde(default = "default_true")]
    pub share_history: bool,
    /// Structured-output strategy for suggest mode: "schema" (strict JSON schema,
    /// default), "json" (any JSON object), or "prompt" (unconstrained).
    #[serde(default = "default_structured")]
    pub structured: String,
    /// Stream answers token-by-token in the interactive REPL (suggest/auto).
    #[serde(default)]
    pub stream: bool,
    /// Print a dim per-session token/cost line after each model interaction.
    #[serde(default = "default_true")]
    pub show_usage: bool,
    /// Stop calling the model once estimated session cost reaches this many USD.
    /// `0` = unlimited. Only enforced when the model's price is known.
    #[serde(default)]
    pub budget_usd: f64,
    /// Remember recent natural-language turns in the interactive REPL so
    /// follow-ups ("now do the same for the other file") have context. Clear it
    /// with `aishe reset`.
    #[serde(default = "default_true")]
    pub memory: bool,
    /// Redact likely secrets (tokens, passwords, URL credentials) from the
    /// environment context block sent to the model. On by default.
    #[serde(default = "default_true")]
    pub redact_secrets: bool,
    /// Inline AI ghost-text autosuggestion in the reedline front-end. Off by
    /// default (it spends tokens as you type). Toggle with `aishe ghost`.
    #[serde(default)]
    pub ghost_text: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Providers {
    #[serde(default = "default_anthropic")]
    pub anthropic: ProviderConfig,
    #[serde(default = "default_openai")]
    pub openai: ProviderConfig,
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            anthropic: default_anthropic(),
            openai: default_openai(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
}

fn default_mode() -> String {
    "suggest".to_string()
}
fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_true() -> bool {
    true
}
fn default_max_iters() -> u32 {
    10
}
fn default_yolo_confirm() -> String {
    crate::sandbox::DEFAULT_CONFIRM.to_string()
}
fn default_cache_ttl() -> u64 {
    300
}
fn default_report_time() -> u64 {
    3
}
fn default_front_end() -> String {
    "auto".to_string()
}
fn default_edit_mode() -> String {
    "emacs".to_string()
}
fn default_structured() -> String {
    "schema".to_string()
}

fn default_anthropic() -> ProviderConfig {
    ProviderConfig {
        base_url: "https://api.anthropic.com".to_string(),
        api_key_env: "ANTHROPIC_API_KEY".to_string(),
        model: "claude-sonnet-4-20250514".to_string(),
    }
}

fn default_openai() -> ProviderConfig {
    ProviderConfig {
        base_url: "https://api.openai.com".to_string(),
        api_key_env: "OPENAI_API_KEY".to_string(),
        model: "gpt-4o".to_string(),
    }
}

impl Default for AisheConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            provider: default_provider(),
            yolo_confirm_dangerous: true,
            yolo_confirm: default_yolo_confirm(),
            yolo_sandbox: false,
            max_yolo_iterations: default_max_iters(),
            yolo_plan: false,
            project_context: true,
            cache: true,
            cache_ttl_secs: default_cache_ttl(),
            file_tools: true,
            web_tool: true,
            show_right_prompt: true,
            front_end: default_front_end(),
            pty_prompt: true,
            edit_mode: default_edit_mode(),
            prompt_format: None,
            git_prompt: true,
            git_status: true,
            report_time: default_report_time(),
            auto_pushd: false,
            hist_ignore_dups: true,
            hist_ignore_space: false,
            hist_ignore: Vec::new(),
            cdpath: Vec::new(),
            correct: false,
            complete_flags: true,
            share_history: true,
            structured: default_structured(),
            stream: false,
            show_usage: true,
            budget_usd: 0.0,
            memory: true,
            redact_secrets: true,
            ghost_text: false,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        default_anthropic()
    }
}

impl Config {
    /// Path to the config file (`~/.config/aishe/config.toml`).
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aishe")
            .join("config.toml")
    }

    /// Legacy config path from before the rename (`~/.config/llmsh/config.toml`).
    fn legacy_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("llmsh")
            .join("config.toml")
    }

    /// Load config, running the first-run wizard if the file is missing.
    /// On a malformed file, offers to back it up and recreate.
    pub fn load_or_init() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            // One-time migration: adopt a pre-rename ~/.config/llmsh config.
            if let Some(cfg) = Self::migrate_legacy(&path)? {
                return Ok(cfg);
            }
            // Only prompt when attached to a terminal. Run from a hook, a pipe,
            // or CI, the wizard would read EOF and silently pick defaults (or
            // hang); instead write a default config and tell the user how to set
            // it up properly.
            let cfg = if std::io::stdin().is_terminal() {
                run_wizard()?
            } else {
                eprintln!(
                    "aishe: no config at {} and not running interactively; \
                     writing defaults. Run `aishe` in a terminal to choose your \
                     provider, model, and endpoint, or edit the file directly.",
                    path.display()
                );
                Config::default()
            };
            cfg.save()?;
            println!("Saved config to {}", path.display());
            return Ok(cfg);
        }
        Self::load_from(&path)
    }

    /// If a legacy `~/.config/llmsh/config.toml` exists (and no aishe config
    /// does yet), port it to the new location, rewriting the `[llmsh]` section
    /// header to `[aishe]`. Returns the loaded config on success.
    fn migrate_legacy(new_path: &Path) -> Result<Option<Self>> {
        let legacy = Self::legacy_path();
        if !legacy.exists() {
            return Ok(None);
        }
        let text = match std::fs::read_to_string(&legacy) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        let cfg = match Self::parse_legacy_text(&text) {
            Ok(c) => c,
            // Leave a malformed legacy file alone; fall through to the wizard.
            Err(_) => return Ok(None),
        };
        cfg.save()?;
        eprintln!(
            "aishe: migrated config from {} to {}",
            legacy.display(),
            new_path.display()
        );
        Ok(Some(cfg))
    }

    /// Parse a pre-rename config file's text. The only structural difference is
    /// the top-level section name (`[llmsh]` → `[aishe]`); all field names are
    /// shared, so a textual rewrite is sufficient.
    fn parse_legacy_text(text: &str) -> Result<Self> {
        let ported = text.replace("[llmsh]", "[aishe]");
        Ok(toml::from_str::<Config>(&ported)?)
    }

    /// Load the config only if present and well-formed; `Ok(None)` if the file
    /// doesn't exist. Never runs the wizard. Used by `aishe doctor`.
    pub fn load_quiet() -> Result<Option<Self>> {
        let path = Self::path();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(Some(toml::from_str::<Config>(&text)?))
    }

    fn load_from(path: &PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        match toml::from_str::<Config>(&text) {
            Ok(cfg) => Ok(cfg),
            Err(e) => {
                eprintln!("Config at {} is malformed: {e}", path.display());
                if prompt_yes_no("Back it up and recreate with the wizard?")? {
                    let backup = path.with_extension("toml.bak");
                    std::fs::rename(path, &backup)
                        .with_context(|| format!("backing up to {}", backup.display()))?;
                    println!("Backed up to {}", backup.display());
                    let cfg = run_wizard()?;
                    cfg.save()?;
                    Ok(cfg)
                } else {
                    anyhow::bail!("cannot continue with malformed config");
                }
            }
        }
    }

    /// Persist the config to disk, creating parent directories as needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(&path, text)
            .with_context(|| format!("writing config {}", path.display()))?;
        Ok(())
    }

    /// The active provider's model name.
    pub fn active_model(&self) -> &str {
        match self.aishe.provider.as_str() {
            "openai" => &self.providers.openai.model,
            _ => &self.providers.anthropic.model,
        }
    }

    /// Set the active provider's model name.
    pub fn set_active_model(&mut self, model: String) {
        match self.aishe.provider.as_str() {
            "openai" => self.providers.openai.model = model,
            _ => self.providers.anthropic.model = model,
        }
    }
}

/// Known OpenAI-compatible services: (key, label, base_url, default_model,
/// key_env). `base_url` is the host root; aishe appends `/v1/chat/completions`,
/// so it must not include `/v1`. "other" is the escape hatch for any endpoint.
const OPENAI_PRESETS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "openai",
        "OpenAI",
        "https://api.openai.com",
        "gpt-4o",
        "OPENAI_API_KEY",
    ),
    (
        "groq",
        "Groq",
        "https://api.groq.com/openai",
        "llama-3.3-70b-versatile",
        "GROQ_API_KEY",
    ),
    (
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api",
        "openai/gpt-4o",
        "OPENROUTER_API_KEY",
    ),
    (
        "together",
        "Together AI",
        "https://api.together.xyz",
        "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        "TOGETHER_API_KEY",
    ),
    (
        "ollama",
        "Ollama (local)",
        "http://localhost:11434",
        "llama3.1",
        "OLLAMA_API_KEY",
    ),
    ("other", "Other / custom endpoint", "", "", "OPENAI_API_KEY"),
];

/// Tidy a user-entered base URL: trim spaces and a trailing slash, and add a
/// scheme if the user omitted one (so "localhost:11434" still works).
fn normalize_base_url(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        "https://api.openai.com".to_string()
    } else if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    }
}

fn run_wizard() -> Result<Config> {
    println!("\n  aishe — first-run setup\n  ───────────────────────");
    let mut cfg = Config::default();

    let provider = prompt_choice(
        "Provider",
        &[
            ("anthropic", "Anthropic (Claude)"),
            (
                "openai",
                "OpenAI-compatible (OpenAI, Groq, OpenRouter, Together, Ollama, …)",
            ),
        ],
        "anthropic",
    )?;
    cfg.aishe.provider = provider.clone();

    // Provider-specific details. For OpenAI-compatible services the endpoint
    // (base URL) is what distinguishes Groq / OpenRouter / Ollama / ... from
    // OpenAI, so always confirm it, pre-filled from the chosen preset.
    if provider == "openai" {
        let preset_opts: Vec<(&str, &str)> = OPENAI_PRESETS.iter().map(|p| (p.0, p.1)).collect();
        let preset = prompt_choice("Service", &preset_opts, "openai")?;
        let row = OPENAI_PRESETS
            .iter()
            .find(|p| p.0 == preset.as_str())
            .copied()
            .unwrap_or(OPENAI_PRESETS[0]);
        let (_, _, def_base, def_model, def_key_env) = row;

        let base_default = if def_base.is_empty() {
            "https://api.openai.com"
        } else {
            def_base
        };
        let base_url = prompt_text(
            &format!("API endpoint (base URL) [{base_default}]"),
            base_default,
        )?;
        cfg.providers.openai.base_url = normalize_base_url(&base_url);

        let key_env = prompt_text(
            &format!("Env var holding your API key [{def_key_env}]"),
            def_key_env,
        )?;
        cfg.providers.openai.api_key_env = key_env;

        let model_default = if def_model.is_empty() {
            "gpt-4o"
        } else {
            def_model
        };
        let model = prompt_text(&format!("Model [{model_default}]"), model_default)?;
        cfg.providers.openai.model = model;
    } else {
        let key_env = prompt_text(
            "Env var holding your API key [ANTHROPIC_API_KEY]",
            "ANTHROPIC_API_KEY",
        )?;
        cfg.providers.anthropic.api_key_env = key_env;

        let default_model = "claude-sonnet-4-20250514";
        let model = prompt_text(&format!("Model [{default_model}]"), default_model)?;
        cfg.providers.anthropic.model = model;
    }

    let mode = prompt_choice(
        "Default mode",
        &[
            ("suggest", "suggest — confirm before running"),
            ("auto", "auto — auto-run safe commands, confirm dangerous"),
            ("yolo", "yolo — autonomous tool loop"),
        ],
        "suggest",
    )?;
    cfg.aishe.mode = mode;

    // Recap what was chosen, and flag a missing API key before they hit it.
    let (active_env, active_base, active_model) = if provider == "openai" {
        (
            cfg.providers.openai.api_key_env.as_str(),
            cfg.providers.openai.base_url.as_str(),
            cfg.providers.openai.model.as_str(),
        )
    } else {
        (
            cfg.providers.anthropic.api_key_env.as_str(),
            cfg.providers.anthropic.base_url.as_str(),
            cfg.providers.anthropic.model.as_str(),
        )
    };
    println!("\n  Summary");
    println!("    provider: {provider}");
    println!("    endpoint: {active_base}");
    println!("    model:    {active_model}");
    println!("    API key:  ${active_env}");

    let key_set = std::env::var(active_env)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if !key_set {
        println!(
            "\n  Note: ${active_env} is not set. Export it before using LLM features:\n    export {active_env}=...\n"
        );
    }

    Ok(cfg)
}

fn prompt_text(label: &str, default: &str) -> Result<String> {
    print!("  {label}: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_string()
    } else {
        line.to_string()
    })
}

fn prompt_choice(label: &str, options: &[(&str, &str)], default: &str) -> Result<String> {
    println!("  {label}:");
    for (i, (key, desc)) in options.iter().enumerate() {
        let marker = if *key == default { "*" } else { " " };
        println!("    {marker} {}) {desc}", i + 1);
    }
    print!("  choose [1-{}] (default {default}): ", options.len());
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(default.to_string());
    }
    if let Ok(n) = line.parse::<usize>() {
        if n >= 1 && n <= options.len() {
            return Ok(options[n - 1].0.to_string());
        }
    }
    // Allow typing the key directly.
    for (key, _) in options {
        if line.eq_ignore_ascii_case(key) {
            return Ok((*key).to_string());
        }
    }
    Ok(default.to_string())
}

fn prompt_yes_no(label: &str) -> Result<bool> {
    print!("  {label} [y/N]: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_url_cases() {
        assert_eq!(
            normalize_base_url("https://api.groq.com/openai/"),
            "https://api.groq.com/openai"
        );
        assert_eq!(normalize_base_url("  https://x.test  "), "https://x.test");
        // A missing scheme gets https://; a bare host:port still works.
        assert_eq!(
            normalize_base_url("localhost:11434"),
            "https://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434"),
            "http://localhost:11434"
        );
        // Empty falls back to the OpenAI default.
        assert_eq!(normalize_base_url("   "), "https://api.openai.com");
    }

    #[test]
    fn openai_presets_are_well_formed() {
        for (key, label, base, _model, key_env) in OPENAI_PRESETS {
            assert!(!key.is_empty() && !label.is_empty() && !key_env.is_empty());
            if *key == "other" {
                continue; // intentionally blank base/model (prompted)
            }
            // base_url is the host root; the provider appends /v1, so presets
            // must not bake it in.
            assert!(base.contains("://"), "{key} base needs a scheme");
            assert!(!base.ends_with("/v1"), "{key} base must not include /v1");
            assert!(!base.ends_with('/'), "{key} base must not end with a slash");
        }
    }

    #[test]
    fn defaults_round_trip() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.aishe.mode, "suggest");
        assert_eq!(parsed.aishe.provider, "anthropic");
        assert_eq!(parsed.providers.anthropic.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(parsed.providers.openai.model, "gpt-4o");
        assert!(parsed.aishe.yolo_confirm_dangerous);
        assert_eq!(parsed.aishe.max_yolo_iterations, 10);
    }

    #[test]
    fn partial_config_fills_defaults() {
        let text = r#"
            [aishe]
            mode = "yolo"
        "#;
        let cfg: Config = toml::from_str(text).unwrap();
        assert_eq!(cfg.aishe.mode, "yolo");
        // Unspecified fields fall back to defaults.
        assert_eq!(cfg.aishe.provider, "anthropic");
        assert_eq!(cfg.providers.openai.base_url, "https://api.openai.com");
    }

    #[test]
    fn legacy_config_is_ported() {
        // A pre-rename config (with the [llmsh] section) migrates to the aishe
        // schema, preserving user settings and filling defaults.
        let legacy = r#"
            [llmsh]
            mode = "yolo"
            provider = "openai"

            [providers.openai]
            base_url = "http://localhost:11434"
            api_key_env = "OPENAI_API_KEY"
            model = "llama3"
        "#;
        let cfg = Config::parse_legacy_text(legacy).unwrap();
        assert_eq!(cfg.aishe.mode, "yolo");
        assert_eq!(cfg.aishe.provider, "openai");
        assert_eq!(cfg.providers.openai.model, "llama3");
        // Unspecified fields still fall back to defaults.
        assert_eq!(cfg.aishe.front_end, "auto");
        assert_eq!(cfg.providers.anthropic.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn active_model_tracks_provider() {
        let mut cfg = Config::default();
        assert_eq!(cfg.active_model(), "claude-sonnet-4-20250514");
        cfg.aishe.provider = "openai".into();
        assert_eq!(cfg.active_model(), "gpt-4o");
        cfg.set_active_model("gpt-4o-mini".into());
        assert_eq!(cfg.providers.openai.model, "gpt-4o-mini");
    }
}
