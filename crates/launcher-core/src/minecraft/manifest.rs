//! Fetching and caching of the version manifest and version JSONs.

use std::path::PathBuf;

use crate::config::Paths;
use crate::{Error, Result};

use super::model::{ManifestVersion, VersionJson, VersionManifest, VERSION_MANIFEST_URL};

fn manifest_cache(paths: &Paths) -> PathBuf {
    paths.meta_dir().join("version_manifest_v2.json")
}

fn version_json_cache(paths: &Paths, id: &str) -> PathBuf {
    paths.meta_dir().join("versions").join(format!("{id}.json"))
}

/// Fetch the version manifest, falling back to the on-disk cache when the
/// network is unavailable.
pub async fn fetch_manifest(paths: &Paths) -> Result<VersionManifest> {
    let cache = manifest_cache(paths);
    match crate::http::client().get(VERSION_MANIFEST_URL).send().await {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().await?;
            let manifest: VersionManifest = serde_json::from_str(&text)?;
            if let Some(parent) = cache.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&cache, &text)?;
            Ok(manifest)
        }
        _ if cache.exists() => {
            let data = std::fs::read_to_string(&cache)?;
            Ok(serde_json::from_str(&data)?)
        }
        Ok(resp) => Err(Error::Other(format!(
            "failed to fetch version manifest (HTTP {})",
            resp.status()
        ))),
        Err(e) => Err(e.into()),
    }
}

pub fn find_version<'a>(
    manifest: &'a VersionManifest,
    id: &str,
) -> Result<&'a ManifestVersion> {
    manifest
        .versions
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| Error::UnknownVersion(id.to_string()))
}

/// Fetch a vanilla version JSON, using the local cache when present.
pub async fn fetch_version_json(paths: &Paths, entry: &ManifestVersion) -> Result<VersionJson> {
    let cache = version_json_cache(paths, &entry.id);
    if cache.exists() {
        let data = std::fs::read_to_string(&cache)?;
        if let Ok(json) = serde_json::from_str(&data) {
            return Ok(json);
        }
    }
    let text = crate::http::client()
        .get(&entry.url)
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

/// Merge a loader profile (child, e.g. Fabric) over its vanilla parent.
/// The child's main class and libraries take precedence; arguments are
/// concatenated (parent first).
pub fn merge_versions(parent: VersionJson, child: VersionJson) -> VersionJson {
    use super::model::maven_group_artifact;
    use std::collections::HashSet;

    // Child libraries win on group:artifact conflicts (e.g. ASM versions).
    let mut seen: HashSet<String> = HashSet::new();
    let mut libraries = Vec::new();
    for lib in child.libraries.into_iter().chain(parent.libraries) {
        if seen.insert(maven_group_artifact(&lib.name)) {
            libraries.push(lib);
        }
    }

    let arguments = match (parent.arguments, child.arguments) {
        (Some(mut p), Some(c)) => {
            p.game.extend(c.game);
            p.jvm.extend(c.jvm);
            Some(p)
        }
        (p, c) => c.or(p),
    };

    VersionJson {
        id: child.id,
        main_class: child.main_class.or(parent.main_class),
        inherits_from: None,
        arguments,
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        asset_index: child.asset_index.or(parent.asset_index),
        assets: child.assets.or(parent.assets),
        downloads: child.downloads.or(parent.downloads),
        libraries,
        java_version: child.java_version.or(parent.java_version),
        kind: child.kind.or(parent.kind),
    }
}
