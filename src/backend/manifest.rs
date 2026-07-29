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
        let mut platforms = std::collections::BTreeSet::new();
        for asset in &self.assets {
            if !platforms.insert(&asset.platform) {
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
        Ok(())
    }

    pub fn platform_key() -> Result<&'static str> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Ok("linux-x86_64"),
            ("linux", "aarch64") => Ok("linux-aarch64"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_is_complete_and_strict() {
        let manifest = RuntimeManifest::embedded().unwrap();
        assert_eq!(manifest.version, "1.18.9");
        assert_eq!(manifest.assets.len(), 4);
        assert!(manifest.assets.iter().all(|asset| asset.size > 40_000_000));
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
