//! Fabric loader support via the Fabric meta API.

use serde::Deserialize;

use crate::config::Paths;
use crate::{Error, Result};

use super::model::VersionJson;

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderEntry {
    pub loader: LoaderInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderInfo {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

/// List loader versions available for a game version (newest first).
pub async fn loader_versions(game_version: &str) -> Result<Vec<LoaderInfo>> {
    let url = format!("{FABRIC_META}/versions/loader/{game_version}");
    let entries: Vec<LoaderEntry> = crate::http::client()
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if entries.is_empty() {
        return Err(Error::Other(format!(
            "Fabric は Minecraft {game_version} に対応していません"
        )));
    }
    Ok(entries.into_iter().map(|e| e.loader).collect())
}

/// Latest stable loader version for a game version.
pub async fn latest_loader(game_version: &str) -> Result<String> {
    let versions = loader_versions(game_version).await?;
    Ok(versions
        .iter()
        .find(|v| v.stable)
        .or_else(|| versions.first())
        .map(|v| v.version.clone())
        .expect("loader_versions returned a non-empty list"))
}

/// Fetch (and cache) the Fabric launcher profile for a game/loader pair.
/// The result is a version JSON with `inheritsFrom` pointing at vanilla.
pub async fn fetch_profile(
    paths: &Paths,
    game_version: &str,
    loader_version: &str,
) -> Result<VersionJson> {
    let cache = paths
        .meta_dir()
        .join("fabric")
        .join(format!("fabric-{loader_version}-{game_version}.json"));
    if cache.exists() {
        if let Ok(json) = serde_json::from_str(&std::fs::read_to_string(&cache)?) {
            return Ok(json);
        }
    }
    let url =
        format!("{FABRIC_META}/versions/loader/{game_version}/{loader_version}/profile/json");
    let text = crate::http::client()
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let json: VersionJson = serde_json::from_str(&text)?;
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cache, &text)?;
    Ok(json)
}
