//! Minimal Modrinth v2 API client (search, versions, file download).

use serde::Deserialize;

use crate::minecraft::install::download_file;
use crate::{Error, Result};
use std::path::{Path, PathBuf};

const API_BASE: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Mod,
    Modpack,
    ResourcePack,
    Shader,
}

impl ProjectType {
    pub fn facet(&self) -> &'static str {
        match self {
            ProjectType::Mod => "project_type:mod",
            ProjectType::Modpack => "project_type:modpack",
            ProjectType::ResourcePack => "project_type:resourcepack",
            ProjectType::Shader => "project_type:shader",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total_hits: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub downloads: u64,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub author: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectVersion {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub files: Vec<VersionFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    pub primary: bool,
    pub size: u64,
    #[serde(default)]
    pub hashes: FileHashes,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileHashes {
    #[serde(default)]
    pub sha1: Option<String>,
}

/// Search projects. `game_version` and `loader` narrow the results via
/// facets when given.
pub async fn search(
    query: &str,
    project_type: ProjectType,
    game_version: Option<&str>,
    loader: Option<&str>,
    limit: u32,
) -> Result<SearchResults> {
    let mut facets: Vec<Vec<String>> = vec![vec![project_type.facet().to_string()]];
    if let Some(v) = game_version {
        facets.push(vec![format!("versions:{v}")]);
    }
    if let Some(l) = loader {
        facets.push(vec![format!("categories:{l}")]);
    }
    let resp = crate::http::client()
        .get(format!("{API_BASE}/search"))
        .query(&[
            ("query", query.to_string()),
            ("limit", limit.to_string()),
            ("index", "relevance".to_string()),
            ("facets", serde_json::to_string(&facets)?),
        ])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(Error::Modrinth(format!(
            "search failed (HTTP {})",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

/// List versions of a project compatible with a game version / loader.
pub async fn project_versions(
    id_or_slug: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Result<Vec<ProjectVersion>> {
    let mut req = crate::http::client().get(format!("{API_BASE}/project/{id_or_slug}/version"));
    if let Some(v) = game_version {
        req = req.query(&[("game_versions", format!("[\"{v}\"]"))]);
    }
    if let Some(l) = loader {
        req = req.query(&[("loaders", format!("[\"{l}\"]"))]);
    }
    let resp = req.send().await?;
    if resp.status().as_u16() == 404 {
        return Err(Error::Modrinth(format!("project not found: {id_or_slug}")));
    }
    if !resp.status().is_success() {
        return Err(Error::Modrinth(format!(
            "version list failed (HTTP {})",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

/// Download the primary file of a version into `dest_dir` (e.g. the
/// instance's `mods/` folder). Returns the path of the downloaded file.
pub async fn download_version(version: &ProjectVersion, dest_dir: &Path) -> Result<PathBuf> {
    let file = version
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| version.files.first())
        .ok_or_else(|| Error::Modrinth(format!("version {} has no files", version.id)))?;
    let dest = dest_dir.join(&file.filename);
    download_file(&file.url, &dest, file.hashes.sha1.as_deref()).await?;
    Ok(dest)
}
