use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const EMBEDDED: &str = include_str!("../../assets/backend/opencode/runtime-manifest.json");

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeManifest {
    pub schema_version: u32,
    pub runtime: String,
    pub version: String,
    pub release_url: String,
    pub assets: Vec<RuntimeAsset>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeAsset {
    pub platform: String,
    pub name: String,
    pub format: String,
    pub size: u64,
    pub sha256: String,
}

impl RuntimeManifest {
    pub fn embedded() -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(EMBEDDED).context("parsing embedded OpenCode runtime manifest")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            anyhow::bail!(
                "unsupported embedded runtime manifest schema {}",
                self.schema_version
            );
        }
        if self.runtime != "opencode" || self.version.trim().is_empty() {
            anyhow::bail!("invalid embedded OpenCode runtime identity");
        }
        if self.assets.is_empty() {
            anyhow::bail!("embedded OpenCode runtime manifest has no assets");
        }
        let mut platforms = std::collections::BTreeSet::<&str>::new();
        for asset in &self.assets {
            if !platforms.insert(asset.platform.as_str()) {
                anyhow::bail!("duplicate runtime platform {}", asset.platform);
            }
            if !matches!(asset.format.as_str(), "tar_gz" | "zip")
                || asset.size == 0
                || asset.sha256.len() != 64
                || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || asset.name.contains('/')
                || asset.name.contains('\\')
            {
                anyhow::bail!("invalid runtime asset for {}", asset.platform);
            }
        }
        let required = [
            "linux-x86_64-gnu",
            "linux-x86_64-musl",
            "linux-aarch64-gnu",
            "linux-aarch64-musl",
            "macos-aarch64",
            "macos-x86_64",
        ]
        .into_iter()
        .collect();
        if platforms != required {
            anyhow::bail!("embedded OpenCode runtime manifest platform set is incomplete");
        }
        Ok(())
    }

    pub fn platform_key() -> Result<&'static str> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", arch @ ("x86_64" | "aarch64")) => {
                linux_platform_key(arch, |path| path.exists())
            }
            ("macos", "aarch64") => Ok("macos-aarch64"),
            ("macos", "x86_64") => Ok("macos-x86_64"),
            (os, arch) => anyhow::bail!("OpenCode runtime is unsupported on {os}-{arch}"),
        }
    }

    pub fn asset_for_current_platform(&self) -> Result<&RuntimeAsset> {
        let platform = Self::platform_key()?;
        self.assets
            .iter()
            .find(|asset| asset.platform == platform)
            .with_context(|| format!("runtime manifest has no asset for {platform}"))
    }

    pub fn source_url(&self, asset: &RuntimeAsset, base_url: Option<&str>) -> String {
        let base = base_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.release_url)
            .trim_end_matches('/');
        format!("{base}/{}", asset.name)
    }
}

fn linux_platform_key(
    arch: &str,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Result<&'static str> {
    let (gnu_key, musl_key, gnu_loaders, musl_loaders): (&str, &str, &[&str], &[&str]) = match arch
    {
        "x86_64" => (
            "linux-x86_64-gnu",
            "linux-x86_64-musl",
            &[
                "/lib64/ld-linux-x86-64.so.2",
                "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
            ],
            &["/lib/ld-musl-x86_64.so.1", "/usr/lib/ld-musl-x86_64.so.1"],
        ),
        "aarch64" => (
            "linux-aarch64-gnu",
            "linux-aarch64-musl",
            &[
                "/lib/ld-linux-aarch64.so.1",
                "/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
            ],
            &["/lib/ld-musl-aarch64.so.1", "/usr/lib/ld-musl-aarch64.so.1"],
        ),
        other => anyhow::bail!("OpenCode runtime is unsupported on linux-{other}"),
    };
    let alpine = exists(std::path::Path::new("/etc/alpine-release"));
    let has_gnu = gnu_loaders
        .iter()
        .any(|path| exists(std::path::Path::new(path)));
    let has_musl = musl_loaders
        .iter()
        .any(|path| exists(std::path::Path::new(path)));
    if has_musl && (alpine || !has_gnu) {
        Ok(musl_key)
    } else {
        // Prefer the glibc build on mainstream Linux. If neither loader can be
        // discovered, installation still performs an executable version probe
        // and fails before activation with an actionable compatibility error.
        Ok(gnu_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_is_complete_and_strict() {
        let manifest = RuntimeManifest::embedded().unwrap();
        assert_eq!(manifest.version, "1.18.9");
        assert_eq!(manifest.assets.len(), 6);
        assert!(manifest.assets.iter().all(|asset| asset.size > 40_000_000));
    }

    #[test]
    fn linux_runtime_selection_prefers_the_native_libc() {
        assert_eq!(
            linux_platform_key("x86_64", |path| {
                path == std::path::Path::new("/lib64/ld-linux-x86-64.so.2")
            })
            .unwrap(),
            "linux-x86_64-gnu"
        );
        assert_eq!(
            linux_platform_key("x86_64", |path| {
                path == std::path::Path::new("/lib/ld-musl-x86_64.so.1")
            })
            .unwrap(),
            "linux-x86_64-musl"
        );
        assert_eq!(
            linux_platform_key("x86_64", |path| {
                matches!(
                    path.to_str(),
                    Some(
                        "/etc/alpine-release"
                            | "/lib64/ld-linux-x86-64.so.2"
                            | "/lib/ld-musl-x86_64.so.1"
                    )
                )
            })
            .unwrap(),
            "linux-x86_64-musl"
        );
        assert_eq!(
            linux_platform_key("aarch64", |path| {
                path == std::path::Path::new("/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1")
            })
            .unwrap(),
            "linux-aarch64-gnu"
        );
    }

    #[test]
    fn source_url_never_uses_latest() {
        let manifest = RuntimeManifest::embedded().unwrap();
        let asset = &manifest.assets[0];
        let url = manifest.source_url(asset, None);
        assert!(url.contains("/v1.18.9/"));
        assert!(!url.contains("/latest/"));
        assert_eq!(
            manifest.source_url(asset, Some("https://mirror.invalid/runtime/")),
            format!("https://mirror.invalid/runtime/{}", asset.name)
        );
    }
}
