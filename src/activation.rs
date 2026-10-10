//! Explicit, reversible native terminal activation. Preview is the default.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const START: &str = "# >>> AIShe native shell >>>";
const END: &str = "# <<< AIShe native shell <<<";

pub(crate) fn run(
    shell: &str,
    apply: bool,
    remove: bool,
    rcfile: Option<&Path>,
    json: bool,
) -> Result<u8> {
    let path = match rcfile {
        Some(path) => path.to_path_buf(),
        None => default_rcfile(shell)?,
    };
    let binary = std::env::current_exe().context("cannot resolve the AIShe executable")?;
    let binary = binary
        .to_str()
        .context("AIShe executable path is not UTF-8")?;
    if binary.chars().any(char::is_control) {
        anyhow::bail!("AIShe executable path contains control characters");
    }
    let quoted = format!("'{}'", binary.replace('\'', "'\\''"));
    let block = format!(
        "{START}\n\
         # Interactive terminals only; the nested AIShe shell skips this block.\n\
         case $- in\n\
         \x20 *i*)\n\
         \x20   if [ -t 0 ] && [ -t 1 ] && [ -z \"${{AISHE_SHELL_ID-}}\" ] && [ -x {quoted} ]; then\n\
         \x20     exec {quoted} -i\n\
         \x20   fi\n\
         \x20   ;;\n\
         esac\n\
         {END}\n"
    );
    let operation = if remove {
        "remove"
    } else if apply {
        "apply"
    } else {
        "preview"
    };
    let mut changed = false;
    let mut backup = None;
    if apply || remove {
        let original = read_startup(&path)?;
        let proposed = revised_startup(&original, &block, remove)?;
        if proposed != original {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).context("cannot create the startup-file directory")?;
            }
            if path.exists() {
                backup = Some(write_backup(&path, original.as_bytes())?);
            }
            if read_startup(&path)? != original {
                anyhow::bail!("startup file changed during activation; retry after reviewing it");
            }
            let permissions = fs::metadata(&path)
                .map(|metadata| metadata.permissions().mode())
                .unwrap_or(0o600);
            write_startup_atomic(&path, proposed.as_bytes(), permissions)?;
            changed = true;
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "operation": operation,
                "shell": shell,
                "path": path,
                "block": block,
                "changed": changed,
                "backup": backup,
                "login_shell_changed": false,
            }))?
        );
    } else if !apply && !remove {
        println!("Native AIShe activation for {}\n\n{block}", path.display());
        println!("Review the block, then run `aishe activate {shell} --apply`.");
        if rcfile.is_some() {
            println!("Include the same `--rcfile` path when applying or removing it.");
        }
        println!("Undo with `aishe activate {shell} --remove`; your login shell stays unchanged.");
    } else {
        println!(
            "{} {}.",
            if changed {
                "Updated"
            } else {
                "Already current:"
            },
            path.display()
        );
        if let Some(backup) = backup {
            println!("Private backup: {}", backup.display());
        }
        if apply {
            println!("Open a new terminal to use AIShe. Undo: `aishe activate {shell} --remove`.");
        }
    }
    Ok(0)
}

fn default_rcfile(shell: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot find your home directory")?;
    Ok(if shell == "zsh" {
        std::env::var_os("ZDOTDIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or(home)
            .join(".zshrc")
    } else {
        home.join(".bashrc")
    })
}

fn read_startup(path: &Path) -> Result<String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!(
                "{} is a symlink; review its destination and use --rcfile with that real path",
                path.display()
            );
        }
        Ok(metadata) if !metadata.is_file() => {
            anyhow::bail!("{} is not a regular startup file", path.display());
        }
        Ok(_) => fs::read_to_string(path).context("cannot read the startup file as UTF-8"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).context("cannot inspect the startup file"),
    }
}

fn revised_startup(original: &str, block: &str, remove: bool) -> Result<String> {
    let starts = original.match_indices(START).collect::<Vec<_>>();
    let ends = original.match_indices(END).collect::<Vec<_>>();
    let mut output = match (starts.as_slice(), ends.as_slice()) {
        ([], []) => {
            if remove {
                return Ok(original.to_string());
            }
            // Enter AIShe before user startup code runs in the outer shell;
            // the real inner shell then loads that code exactly once.
            String::new()
        }
        ([(start, _)], [(end, _)]) if start < end => {
            // Markers must occupy complete lines; never remove arbitrary user text.
            if (*start > 0 && original.as_bytes()[start - 1] != b'\n')
                || (*end > 0 && original.as_bytes()[end - 1] != b'\n')
            {
                anyhow::bail!(
                    "activation markers are embedded in other text; review the startup file"
                );
            }
            let after_start = start + START.len();
            if after_start < original.len() && original.as_bytes()[after_start] != b'\n' {
                anyhow::bail!(
                    "activation start marker contains extra text; review the startup file"
                );
            }
            let after = end + END.len();
            if after < original.len() && original.as_bytes()[after] != b'\n' {
                anyhow::bail!("activation end marker contains extra text; review the startup file");
            }
            let after = after + usize::from(original.as_bytes().get(after) == Some(&b'\n'));
            let mut output = original[..*start].to_string();
            if !remove {
                output.push_str(block);
            }
            output.push_str(&original[after..]);
            return Ok(output);
        }
        _ => anyhow::bail!(
            "activation markers are incomplete or duplicated; review the startup file"
        ),
    };
    output.push_str(block);
    output.push_str(original);
    Ok(output)
}

fn write_backup(path: &Path, contents: &[u8]) -> Result<PathBuf> {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".aishe-backup-{}-{suffix}", std::process::id()));
    let backup = PathBuf::from(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&backup)
        .context("cannot create a private startup-file backup")?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(backup)
}

fn write_startup_atomic(path: &Path, contents: &[u8], permissions: u32) -> Result<()> {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".aishe-tmp-{:016x}", rand::random::<u64>()));
    let temporary = PathBuf::from(name);
    // Always remove a partially written temporary file on an error.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let _cleanup = Cleanup(temporary.clone());
    file.write_all(contents)?;
    file.set_permissions(fs::Permissions::from_mode(permissions))?;
    file.sync_all()?;
    fs::rename(&temporary, path).context("cannot replace the startup file")?;
    Ok(())
}
