//! Native lean tools run in the AIShe process, outside the command sandbox.
//! Apply the turn's explicit workspace authority before dispatching them.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::agent::{ExecutionScope, NetworkPolicy};

#[cfg(test)]
fn execute(
    scope: Option<&(ExecutionScope, PathBuf, NetworkPolicy)>,
    name: &str,
    args: &Value,
    cwd: &Path,
    confirm_writes: bool,
    preview: bool,
) -> (String, String) {
    execute_rendered(scope, name, args, cwd, confirm_writes, preview, false)
}

pub(crate) fn execute_rendered(
    scope: Option<&(ExecutionScope, PathBuf, NetworkPolicy)>,
    name: &str,
    args: &Value,
    cwd: &Path,
    confirm_writes: bool,
    preview: bool,
    show_diff: bool,
) -> (String, String) {
    let Some((ExecutionScope::Workspace, workspace, network)) = scope else {
        return crate::tools::execute_rendered(name, args, cwd, confirm_writes, preview, show_diff);
    };
    match scoped_arguments(workspace, *network, name, args, cwd) {
        Ok(args) => crate::tools::execute_rendered(name, &args, cwd, false, preview, show_diff),
        Err(error) => (
            name.into(),
            format!("Error: workspace scope refused this tool: {error}"),
        ),
    }
}

fn scoped_arguments(
    workspace: &Path,
    network: NetworkPolicy,
    name: &str,
    args: &Value,
    cwd: &Path,
) -> Result<Value> {
    if name == "fetch_url" {
        if network == NetworkPolicy::Deny {
            anyhow::bail!("network access is denied for this workspace session");
        }
        return Ok(args.clone());
    }
    if !crate::tools::is_file_tool(name) {
        anyhow::bail!("unsupported built-in tool '{name}'");
    }
    // A grant stores a canonical root. Re-resolving it must not silently move
    // the authority when that directory is replaced by a symlink later.
    if workspace
        .canonicalize()
        .context("workspace is unavailable")?
        != workspace
    {
        anyhow::bail!("the accepted workspace root changed");
    }
    let cwd = cwd
        .canonicalize()
        .context("working directory is unavailable")?;
    if !cwd.starts_with(workspace) {
        anyhow::bail!("working directory escapes the accepted workspace");
    }
    let value = args.get("path").and_then(Value::as_str).unwrap_or("");
    let value = if value.is_empty() && name == "list_dir" {
        "."
    } else {
        value
    };
    if value.is_empty() || value.contains('\0') {
        anyhow::bail!("file path is invalid");
    }
    let candidate = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        cwd.join(value)
    };
    let write = matches!(name, "write_file" | "edit_file");
    let canonical = if write {
        canonical_with_missing_suffix(&candidate)?
    } else {
        candidate
            .canonicalize()
            .context("file path is unavailable")?
    };
    if !canonical.starts_with(workspace) {
        anyhow::bail!("file path escapes the accepted workspace");
    }
    // Dispatch through the resolved parent, rather than checking a symlink and
    // then traversing the original alias again. Preserve the existing tool's
    // final-component replacement behavior for write_file / edit_file.
    let resolved = if write {
        let parent = candidate.parent().context("file target has no parent")?;
        let filename = candidate
            .file_name()
            .context("file target has no file name")?;
        let parent = canonical_with_missing_suffix(parent)?;
        if !parent.starts_with(workspace) {
            anyhow::bail!("file parent escapes the accepted workspace");
        }
        parent.join(filename)
    } else {
        canonical
    };
    let mut args = args.clone();
    args.as_object_mut()
        .context("tool arguments must be an object")?
        .insert(
            "path".into(),
            Value::String(resolved.to_string_lossy().into_owned()),
        );
    Ok(args)
}

/// Resolve existing components before appending a missing suffix. Resolving
/// symlinks before `..` matters: lexical normalization can turn an escaping
/// `link/../file` into an apparently safe path. A dangling symlink fails closed.
fn canonical_with_missing_suffix(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        anyhow::bail!("file path must be absolute");
    }
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => (),
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => {
                resolved.push(part);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(_) => {
                        resolved = resolved
                            .canonicalize()
                            .context("file path cannot be resolved")?
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error).context("file path cannot be inspected"),
                }
            }
            Component::Prefix(_) => anyhow::bail!("unsupported file path prefix"),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (Fixture, PathBuf, PathBuf) {
        let temporary = Fixture(std::env::temp_dir().join(format!(
            "aishe-workspace-tools-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        )));
        let workspace = temporary.0.join("project");
        let outside = temporary.0.join("outside");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        (
            temporary,
            workspace.canonicalize().unwrap(),
            outside.canonicalize().unwrap(),
        )
    }

    #[test]
    fn workspace_builtin_writes_cannot_escape_absolute_or_relative_paths() {
        let (_temporary, workspace, outside) = fixture();
        let scope = (
            ExecutionScope::Workspace,
            workspace.clone(),
            NetworkPolicy::Deny,
        );
        std::fs::write(outside.join("existing"), "original").unwrap();
        for path in [
            outside.join("created"),
            outside.join("existing"),
            PathBuf::from("../outside/created"),
        ] {
            let (_, content) = execute(
                Some(&scope),
                "write_file",
                &json!({"path":path,"content":"changed"}),
                &workspace,
                false,
                false,
            );
            assert!(content.contains("workspace scope refused"), "{content}");
        }
        let (_, content) = execute(
            Some(&scope),
            "edit_file",
            &json!({"path":"../outside/existing","find":"original","replace":"changed"}),
            &workspace,
            false,
            false,
        );
        assert!(content.contains("workspace scope refused"), "{content}");
        assert!(!outside.join("created").exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("existing")).unwrap(),
            "original"
        );
        let (_, content) = execute(
            Some(&scope),
            "write_file",
            &json!({"path":"src/new.txt","content":"inside"}),
            &workspace,
            true,
            false,
        );
        assert!(content.starts_with("Wrote"), "{content}");
        assert_eq!(
            std::fs::read_to_string(workspace.join("src/new.txt")).unwrap(),
            "inside"
        );
    }

    #[test]
    fn workspace_root_stays_fixed_when_cwd_changes() {
        let (_temporary, workspace, outside) = fixture();
        let scope = (
            ExecutionScope::Workspace,
            workspace.clone(),
            NetworkPolicy::Deny,
        );
        let (_, content) = execute(
            Some(&scope),
            "write_file",
            &json!({"path":"new","content":"changed"}),
            &outside,
            false,
            false,
        );
        assert!(content.contains("working directory escapes"), "{content}");
        assert!(!outside.join("new").exists());
        let (_, content) = execute(
            Some(&scope),
            "write_file",
            &json!({"path":"new","content":"inside"}),
            &workspace.join("src"),
            false,
            false,
        );
        assert!(content.starts_with("Wrote"), "{content}");
        assert!(workspace.join("src/new").is_file());
    }

    #[test]
    fn nearest_existing_ancestor_handles_new_files_and_missing_directories() {
        let (_temporary, workspace, outside) = fixture();
        let args = scoped_arguments(
            &workspace,
            NetworkPolicy::Deny,
            "write_file",
            &json!({"path":"new/sub/file"}),
            &workspace,
        )
        .unwrap();
        assert_eq!(
            args["path"],
            workspace.join("new/sub/file").to_string_lossy().as_ref()
        );
        assert!(scoped_arguments(
            &workspace,
            NetworkPolicy::Deny,
            "write_file",
            &json!({"path":"../outside/new/sub/file"}),
            &workspace
        )
        .is_err());
        assert!(!outside.join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_and_dangling_symlinks_fail_closed() {
        use std::os::unix::fs::symlink;
        let (_temporary, workspace, outside) = fixture();
        std::fs::write(outside.join("existing"), "original").unwrap();
        symlink(&outside, workspace.join("link")).unwrap();
        symlink(outside.join("existing"), workspace.join("file-link")).unwrap();
        symlink(outside.join("missing"), workspace.join("dangling")).unwrap();
        let scope = (
            ExecutionScope::Workspace,
            workspace.clone(),
            NetworkPolicy::Deny,
        );
        for path in [
            "link/existing",
            "link/new",
            "link/new/sub/file",
            "link/../outside/new",
            "file-link",
            "dangling",
        ] {
            let (_, content) = execute(
                Some(&scope),
                "write_file",
                &json!({"path":path,"content":"changed"}),
                &workspace,
                false,
                false,
            );
            assert!(
                content.contains("workspace scope refused"),
                "{path}: {content}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(outside.join("existing")).unwrap(),
            "original"
        );
        assert!(!outside.join("new").exists());
        assert!(std::fs::symlink_metadata(workspace.join("dangling"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn replacing_the_granted_root_with_a_symlink_does_not_move_authority() {
        use std::os::unix::fs::symlink;
        let (temporary, workspace, outside) = fixture();
        let scope = (
            ExecutionScope::Workspace,
            workspace.clone(),
            NetworkPolicy::Deny,
        );
        std::fs::rename(&workspace, temporary.0.join("retired-project")).unwrap();
        symlink(&outside, &workspace).unwrap();
        let (_, content) = execute(
            Some(&scope),
            "write_file",
            &json!({"path":"new","content":"changed"}),
            &workspace,
            false,
            false,
        );
        assert!(
            content.contains("accepted workspace root changed"),
            "{content}"
        );
        assert!(!outside.join("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_resolved_before_parent_components() {
        use std::os::unix::fs::symlink;
        let (_temporary, workspace, outside) = fixture();
        std::fs::create_dir_all(outside.join("deep")).unwrap();
        symlink(outside.join("deep"), workspace.join("link")).unwrap();
        assert_eq!(
            canonical_with_missing_suffix(&workspace.join("link/../new")).unwrap(),
            outside.join("new")
        );
        assert!(scoped_arguments(
            &workspace,
            NetworkPolicy::Deny,
            "write_file",
            &json!({"path":"link/../new"}),
            &workspace
        )
        .is_err());
        symlink(workspace.join("src"), workspace.join("inside")).unwrap();
        let args = scoped_arguments(
            &workspace,
            NetworkPolicy::Deny,
            "write_file",
            &json!({"path":"inside/new"}),
            &workspace,
        )
        .unwrap();
        assert_eq!(
            args["path"],
            workspace.join("src/new").to_string_lossy().as_ref()
        );
    }

    #[test]
    fn deny_network_refuses_fetch_before_any_request() {
        let (_temporary, workspace, _) = fixture();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let scope = (
            ExecutionScope::Workspace,
            workspace.clone(),
            NetworkPolicy::Deny,
        );
        let args = json!({"url":format!("http://{}/private", listener.local_addr().unwrap())});
        let (_, content) = execute(Some(&scope), "fetch_url", &args, &workspace, false, false);
        assert!(content.contains("network access is denied"), "{content}");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(
            scoped_arguments(
                &workspace,
                NetworkPolicy::Allow,
                "fetch_url",
                &args,
                &workspace
            )
            .unwrap(),
            args
        );
    }

    #[test]
    fn host_and_legacy_tools_preserve_existing_authority() {
        let (_temporary, workspace, outside) = fixture();
        std::fs::write(outside.join("readable"), "host file").unwrap();
        let scope = (ExecutionScope::Host, workspace.clone(), NetworkPolicy::Deny);
        let args = json!({"path":outside.join("readable")});
        assert_eq!(
            execute(Some(&scope), "read_file", &args, &workspace, false, false).1,
            "host file"
        );
        assert_eq!(
            execute(None, "read_file", &args, &workspace, false, false).1,
            "host file"
        );
        let scope = (
            ExecutionScope::Workspace,
            workspace.clone(),
            NetworkPolicy::Deny,
        );
        assert!(
            execute(Some(&scope), "read_file", &args, &workspace, false, false)
                .1
                .contains("workspace scope refused")
        );
        assert!(execute(
            Some(&scope),
            "list_dir",
            &json!({}),
            &workspace,
            false,
            false
        )
        .1
        .contains("src/"));
    }
}
