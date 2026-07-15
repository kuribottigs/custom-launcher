//! Downloading of client jars, libraries (including native extraction) and
//! assets, with SHA-1 verification and parallelism.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use futures::stream::{self, StreamExt};
use sha1::{Digest, Sha1};

use crate::config::Paths;
use crate::progress::Progress;
use crate::{Error, Result};

use super::model::{
    maven_coordinate_to_path, AssetIndex, Library, VersionJson, DEFAULT_MAVEN_URL, RESOURCES_URL,
};
use super::rules::{rules_allow, os_name};

const CONCURRENT_DOWNLOADS: usize = 16;

fn file_sha1(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Download `url` to `dest`, skipping when the file already exists with a
/// matching SHA-1 (or any content, when no hash is known).
pub async fn download_file(url: &str, dest: &Path, sha1: Option<&str>) -> Result<()> {
    if dest.exists() {
        match sha1 {
            Some(expected) => {
                if file_sha1(dest)?.eq_ignore_ascii_case(expected) {
                    return Ok(());
                }
            }
            None => return Ok(()),
        }
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = crate::http::client()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if let Some(expected) = sha1 {
        let actual = hex::encode(Sha1::digest(&bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::ChecksumMismatch {
                path: dest.display().to_string(),
                expected: expected.to_string(),
                actual,
            });
        }
    }
    let tmp = dest.with_extension("part");
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

struct PlannedDownload {
    url: String,
    dest: PathBuf,
    sha1: Option<String>,
    /// Set when this jar contains native libraries to extract.
    extract_to: Option<(PathBuf, Vec<String>)>,
}

/// Resolve a library to its download URL and repository-relative path.
fn resolve_library(lib: &Library) -> Option<(String, String, Option<String>)> {
    if let Some(downloads) = &lib.downloads {
        if let Some(artifact) = &downloads.artifact {
            let path = artifact
                .path
                .clone()
                .or_else(|| maven_coordinate_to_path(&lib.name))?;
            return Some((artifact.url.clone(), path, artifact.sha1.clone()));
        }
        // Natives-only libraries (handled separately via classifiers).
        if downloads.classifiers.is_some() {
            return None;
        }
    }
    let path = maven_coordinate_to_path(&lib.name)?;
    let base = lib.url.as_deref().unwrap_or(DEFAULT_MAVEN_URL);
    let base = base.trim_end_matches('/');
    Some((format!("{base}/{path}"), path, lib.sha1.clone()))
}

/// The native classifier for the current OS (legacy pre-1.19 format),
/// with `${arch}` expanded.
fn native_classifier(lib: &Library) -> Option<String> {
    let natives = lib.natives.as_ref()?;
    let key = natives.get(os_name())?;
    let bits = if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    };
    Some(key.replace("${arch}", bits))
}

fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(jar)?;
    let mut archive = zip::ZipArchive::new(file)?;
    'entry: for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        for pattern in exclude {
            if name.starts_with(pattern.trim_end_matches('/')) {
                continue 'entry;
            }
        }
        // Natives jars are flat; keep only the file name to be safe against
        // path traversal.
        let file_name = Path::new(&name).file_name().unwrap_or_default();
        if file_name.is_empty() {
            continue;
        }
        let out = dest.join(file_name);
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        std::fs::write(out, data)?;
    }
    Ok(())
}

pub struct Installer {
    pub paths: Paths,
    pub progress: Progress,
    /// Base URL for asset objects; overridable for tests/mirrors.
    pub resources_base: String,
}

impl Installer {
    pub fn new(paths: Paths, progress: Progress) -> Self {
        Self {
            paths,
            progress,
            resources_base: RESOURCES_URL.to_string(),
        }
    }

    async fn run_downloads(&self, plans: Vec<PlannedDownload>, stage: &str) -> Result<()> {
        let total = plans.len();
        if total == 0 {
            return Ok(());
        }
        self.progress.stage(stage.to_string());
        let done = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<Result<()>> = stream::iter(plans)
            .map(|plan| {
                let progress = self.progress.clone();
                let done = &done;
                async move {
                    download_file(&plan.url, &plan.dest, plan.sha1.as_deref()).await?;
                    if let Some((dir, exclude)) = &plan.extract_to {
                        let jar = plan.dest.clone();
                        let dir = dir.clone();
                        let exclude = exclude.clone();
                        tokio::task::spawn_blocking(move || extract_natives(&jar, &dir, &exclude))
                            .await
                            .map_err(|e| Error::Other(e.to_string()))??;
                    }
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    progress.progress(n, total);
                    Ok(())
                }
            })
            .buffer_unordered(CONCURRENT_DOWNLOADS)
            .collect()
            .await;
        for result in results {
            result?;
        }
        Ok(())
    }

    /// Download the client jar for `version` (stored under the vanilla
    /// version id when the profile inherits from one).
    pub async fn install_client_jar(&self, version: &VersionJson, jar_id: &str) -> Result<PathBuf> {
        let dest = self.paths.client_jar(jar_id);
        let client = version
            .downloads
            .as_ref()
            .and_then(|d| d.client.as_ref())
            .ok_or_else(|| Error::Other(format!("version {} has no client download", version.id)))?;
        self.progress.stage("クライアント本体をダウンロード中");
        download_file(&client.url, &dest, client.sha1.as_deref()).await?;
        Ok(dest)
    }

    /// Download all applicable libraries and extract natives.
    /// Returns the classpath entries.
    pub async fn install_libraries(
        &self,
        version: &VersionJson,
        natives_id: &str,
    ) -> Result<Vec<PathBuf>> {
        let features = HashSet::new();
        let lib_dir = self.paths.libraries_dir();
        let natives_dir = self.paths.natives_dir(natives_id);
        let mut plans = Vec::new();
        let mut classpath = Vec::new();

        for lib in &version.libraries {
            if !rules_allow(lib.rules.as_deref(), &features) {
                continue;
            }
            if let Some((url, rel_path, sha1)) = resolve_library(lib) {
                let dest = lib_dir.join(&rel_path);
                classpath.push(dest.clone());
                plans.push(PlannedDownload {
                    url,
                    dest,
                    sha1,
                    extract_to: None,
                });
            }
            // Legacy natives via classifiers.
            if let Some(classifier) = native_classifier(lib) {
                let artifact = lib
                    .downloads
                    .as_ref()
                    .and_then(|d| d.classifiers.as_ref())
                    .and_then(|c| c.get(&classifier));
                if let Some(artifact) = artifact {
                    let rel_path = artifact
                        .path
                        .clone()
                        .or_else(|| {
                            maven_coordinate_to_path(&format!("{}:{classifier}", lib.name))
                        })
                        .ok_or_else(|| {
                            Error::Other(format!("cannot resolve natives path for {}", lib.name))
                        })?;
                    let exclude = lib
                        .extract
                        .as_ref()
                        .map(|e| e.exclude.clone())
                        .unwrap_or_default();
                    plans.push(PlannedDownload {
                        url: artifact.url.clone(),
                        dest: lib_dir.join(&rel_path),
                        sha1: artifact.sha1.clone(),
                        extract_to: Some((natives_dir.clone(), exclude)),
                    });
                }
            }
        }
        std::fs::create_dir_all(&natives_dir)?;
        self.run_downloads(plans, "ライブラリをダウンロード中").await?;
        Ok(classpath)
    }

    /// Download the asset index and all asset objects.
    pub async fn install_assets(&self, version: &VersionJson) -> Result<()> {
        let Some(index_ref) = &version.asset_index else {
            return Ok(());
        };
        let assets_dir = self.paths.assets_dir();
        let index_path = assets_dir
            .join("indexes")
            .join(format!("{}.json", index_ref.id));
        download_file(&index_ref.url, &index_path, index_ref.sha1.as_deref()).await?;
        let index: AssetIndex = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;

        let mut plans = Vec::new();
        for obj in index.objects.values() {
            let prefix = &obj.hash[..2];
            let dest = assets_dir.join("objects").join(prefix).join(&obj.hash);
            plans.push(PlannedDownload {
                url: format!("{}/{prefix}/{}", self.resources_base, obj.hash),
                dest,
                sha1: Some(obj.hash.clone()),
                extract_to: None,
            });
        }
        self.run_downloads(plans, "アセットをダウンロード中").await?;

        // Legacy versions read assets from a plain directory tree.
        if index.virtual_.unwrap_or(false) || index.map_to_resources.unwrap_or(false) {
            let virtual_dir = assets_dir.join("virtual").join(&index_ref.id);
            for (name, obj) in &index.objects {
                let src = assets_dir
                    .join("objects")
                    .join(&obj.hash[..2])
                    .join(&obj.hash);
                let dest = virtual_dir.join(name);
                if !dest.exists() {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(&src, &dest)?;
                }
            }
        }
        Ok(())
    }
}
