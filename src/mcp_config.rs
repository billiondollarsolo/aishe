//! Transactional MCP configuration commands with environment-name references.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::config::{Config, McpServerConfig};

pub struct ServerInput {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: Vec<String>,
    pub header_env: Vec<String>,
}

pub fn list(config: &Config, json: bool) -> Result<u8> {
    if json {
        let servers = config
            .mcp_servers
            .iter()
            .map(|(name, server)| {
                serde_json::json!({
                    "name": name,
                    "enabled": server.enabled,
                    "transport": if server.url.is_some() { "http" } else { "stdio" },
                    "command": server.command,
                    "args": server.args,
                    "url": server.url,
                    "environment_names": server.env.keys().collect::<Vec<_>>(),
                    "header_names": server.headers.keys().collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "servers": servers,
            }))?
        );
    } else if config.mcp_servers.is_empty() {
        println!("no MCP servers configured");
    } else {
        for (name, server) in &config.mcp_servers {
            println!(
                "{}  {}  {}",
                name,
                if server.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                server
                    .url
                    .as_deref()
                    .or(server.command.as_deref())
                    .unwrap_or("invalid")
            );
        }
    }
    Ok(0)
}

pub fn show(config: &Config, name: &str, json: bool) -> Result<u8> {
    let server = config
        .mcp_servers
        .get(name)
        .with_context(|| format!("unknown MCP server '{name}'"))?;
    let environment = redacted_references(&server.env);
    let headers = redacted_references(&server.headers);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "name": name,
                "enabled": server.enabled,
                "command": server.command,
                "args": server.args,
                "url": server.url,
                "environment": environment,
                "headers": headers,
            }))?
        );
    } else {
        println!("name: {name}");
        println!("enabled: {}", server.enabled);
        if let Some(command) = &server.command {
            println!("command: {command} {}", server.args.join(" "));
        }
        if let Some(url) = &server.url {
            println!("url: {url}");
        }
        for (name, reference) in &environment {
            println!("env: {name} <- {reference}");
        }
        for (name, reference) in &headers {
            println!("header: {name} <- {reference}");
        }
    }
    Ok(0)
}

pub fn put(name: &str, input: ServerInput, replace: bool, require_existing: bool) -> Result<u8> {
    validate_name(name)?;
    if input.command.is_some() == input.url.is_some() {
        anyhow::bail!("select exactly one of --command or --url");
    }
    if input
        .command
        .as_deref()
        .is_some_and(|command| command.trim().is_empty())
    {
        anyhow::bail!("MCP command cannot be empty");
    }
    if let Some(url) = input.url.as_deref() {
        let parsed = url::Url::parse(url).context("invalid MCP URL")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            anyhow::bail!("MCP URL must be an absolute http(s) URL");
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            anyhow::bail!("MCP URL must not contain credentials; use --header-env");
        }
    }
    let env = references(input.env, "environment", valid_env)?;
    let headers = references(input.header_env, "header", valid_header)?;
    let mut config = Config::load_quiet()?.unwrap_or_default();
    let exists = config.mcp_servers.contains_key(name);
    if require_existing && !exists {
        anyhow::bail!("unknown MCP server '{name}'");
    }
    if exists && !replace && !require_existing {
        anyhow::bail!("MCP server '{name}' already exists; use `mcp edit`");
    }
    config.mcp_servers.insert(
        name.to_string(),
        McpServerConfig {
            command: input.command,
            args: input.args,
            env,
            url: input.url,
            headers,
            enabled: config
                .mcp_servers
                .get(name)
                .map(|server| server.enabled)
                .unwrap_or(true),
        },
    );
    config.save()?;
    println!(
        "{} MCP server {name}",
        if exists { "updated" } else { "added" }
    );
    Ok(0)
}

pub fn remove(name: &str) -> Result<u8> {
    validate_name(name)?;
    let mut config = Config::load_quiet()?.unwrap_or_default();
    if config.mcp_servers.remove(name).is_none() {
        anyhow::bail!("unknown MCP server '{name}'");
    }
    config.save()?;
    println!("removed MCP server {name}");
    Ok(0)
}

pub fn enable(name: &str, enabled: bool) -> Result<u8> {
    validate_name(name)?;
    let mut config = Config::load_quiet()?.unwrap_or_default();
    let server = config
        .mcp_servers
        .get_mut(name)
        .with_context(|| format!("unknown MCP server '{name}'"))?;
    server.enabled = enabled;
    config.save()?;
    println!(
        "{} MCP server {name}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(0)
}

pub fn test(config: &Config, name: &str, json: bool) -> Result<u8> {
    let server = config
        .mcp_servers
        .get(name)
        .with_context(|| format!("unknown MCP server '{name}'"))?;
    let mut one = BTreeMap::new();
    one.insert(name.to_string(), server.clone());
    let registry = crate::mcp::McpRegistry::connect(&one);
    let ok = !registry.is_fully_empty();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "name": name,
                "healthy": ok,
                "tools": registry.list(),
                "prompts": registry.list_prompts(),
            }))?
        );
    } else if ok {
        println!("MCP server {name} is healthy");
        crate::cli::runtime::print_mcp_listing(&registry);
    } else {
        eprintln!("MCP server {name} did not expose a healthy capability surface");
    }
    Ok(if ok { 0 } else { 1 })
}

fn references(
    values: Vec<String>,
    kind: &str,
    valid_target: impl Fn(&str) -> bool,
) -> Result<BTreeMap<String, String>> {
    values
        .into_iter()
        .map(|value| {
            let (target, source) = value
                .split_once('=')
                .with_context(|| format!("{kind} reference must be TARGET=ENV_VAR"))?;
            if !valid_target(target.trim()) || !valid_env(source) {
                anyhow::bail!("invalid {kind} reference '{value}'");
            }
            Ok((target.trim().to_string(), format!("env:{}", source.trim())))
        })
        .collect()
}

fn redacted_references(values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                if value.starts_with("env:") {
                    value.clone()
                } else {
                    "<redacted legacy literal>".into()
                },
            )
        })
        .collect()
}

fn valid_env(value: &str) -> bool {
    let mut bytes = value.trim().bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn valid_header(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn validate_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        anyhow::bail!("MCP server name must contain only letters, digits, '-' or '_'");
    }
    Ok(())
}
