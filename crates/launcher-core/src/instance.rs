//! Instance management and the high-level install/launch orchestration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::auth::Account;
use crate::config::{write_json_atomic, Paths, Settings};
use crate::minecraft::{fabric, install::Installer, java, launch, manifest};
use crate::progress::Progress;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoaderConfig {
    pub kind: LoaderKind,
    /// Loader version; resolved to the latest stable at install time when unset.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub minecraft_version: String,
    #[serde(default)]
    pub loader: Option<LoaderConfig>,
    /// Per-instance memory override (MiB).
    #[serde(default)]
    pub memory_mb: Option<u32>,
}

impl Instance {
    pub fn dir(&self, paths: &Paths) -> PathBuf {
        paths.instance_dir(&self.name)
    }

    /// The game directory (`.minecraft`) of this instance.
    pub fn game_dir(&self, paths: &Paths) -> PathBuf {
        self.dir(paths).join(".minecraft")
    }

    pub fn mods_dir(&self, paths: &Paths) -> PathBuf {
        self.game_dir(paths).join("mods")
    }

    pub fn loader_name(&self) -> Option<&'static str> {
        match self.loader.as_ref().map(|l| l.kind) {
            Some(LoaderKind::Fabric) => Some("fabric"),
            None => None,
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json_atomic(&self.dir(paths).join("instance.json"), self)
    }
}

fn valid_instance_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

pub fn create_instance(
    paths: &Paths,
    name: &str,
    minecraft_version: &str,
    loader: Option<LoaderConfig>,
) -> Result<Instance> {
    if !valid_instance_name(name) {
        return Err(Error::Other(format!(
            "インスタンス名に使えない文字が含まれています: {name}"
        )));
    }
    let dir = paths.instance_dir(name);
    if dir.join("instance.json").exists() {
        return Err(Error::InstanceExists(name.to_string()));
    }
    let instance = Instance {
        name: name.to_string(),
        minecraft_version: minecraft_version.to_string(),
        loader,
        memory_mb: None,
    };
    std::fs::create_dir_all(instance.game_dir(paths))?;
    instance.save(paths)?;
    Ok(instance)
}

pub fn list_instances(paths: &Paths) -> Result<Vec<Instance>> {
    let dir = paths.instances_dir();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let file = entry.path().join("instance.json");
            if file.exists() {
                if let Ok(instance) =
                    serde_json::from_str::<Instance>(&std::fs::read_to_string(&file)?)
                {
                    out.push(instance);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn load_instance(paths: &Paths, name: &str) -> Result<Instance> {
    let file = paths.instance_dir(name).join("instance.json");
    if !file.exists() {
        return Err(Error::InstanceNotFound(name.to_string()));
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(&file)?)?)
}

pub fn delete_instance(paths: &Paths, name: &str) -> Result<()> {
    let dir = paths.instance_dir(name);
    if !dir.join("instance.json").exists() {
        return Err(Error::InstanceNotFound(name.to_string()));
    }
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

/// Everything needed to launch, produced by [`prepare`].
pub struct Prepared {
    pub version: crate::minecraft::model::VersionJson,
    /// Version id owning the client jar / natives (vanilla id).
    pub jar_id: String,
    pub classpath: Vec<PathBuf>,
    pub java: java::JavaInstall,
}

/// Resolve the version (merging the Fabric profile when configured),
/// download everything, and locate Java.
pub async fn prepare(
    paths: &Paths,
    settings: &Settings,
    instance: &Instance,
    progress: Progress,
) -> Result<Prepared> {
    progress.stage("バージョン情報を取得中");
    let manifest_data = manifest::fetch_manifest(paths).await?;
    let entry = manifest::find_version(&manifest_data, &instance.minecraft_version)?;
    let vanilla = manifest::fetch_version_json(paths, entry).await?;
    let jar_id = vanilla.id.clone();

    let version = match &instance.loader {
        Some(cfg) => {
            let loader_version = match &cfg.version {
                Some(v) => v.clone(),
                None => fabric::latest_loader(&instance.minecraft_version).await?,
            };
            progress.message(format!("Fabric loader {loader_version} を使用"));
            let profile =
                fabric::fetch_profile(paths, &instance.minecraft_version, &loader_version).await?;
            manifest::merge_versions(vanilla, profile)
        }
        None => vanilla,
    };

    let installer = Installer::new(paths.clone(), progress.clone());
    installer.install_client_jar(&version, &jar_id).await?;
    let classpath = installer.install_libraries(&version, &jar_id).await?;
    installer.install_assets(&version).await?;

    progress.stage("Javaを検索中");
    let required = version.java_version.as_ref().map(|j| j.major_version);
    let java = java::find_java(settings.java_path.as_deref(), required)?;
    if let Some(required) = required {
        if java.major_version < required {
            progress.message(format!(
                "警告: Java {required} が必要ですが {} が見つかりました",
                java.major_version
            ));
        }
    }
    Ok(Prepared {
        version,
        jar_id,
        classpath,
        java,
    })
}

/// Launch a prepared instance. Returns the child process.
pub fn launch_prepared(
    paths: &Paths,
    settings: &Settings,
    instance: &Instance,
    prepared: &Prepared,
    account: &Account,
) -> Result<std::process::Child> {
    let game_dir = instance.game_dir(paths);
    let ctx = launch::LaunchContext {
        paths,
        version: &prepared.version,
        jar_id: &prepared.jar_id,
        game_dir: &game_dir,
        classpath: &prepared.classpath,
        account,
        java: &prepared.java.path,
        memory_mb: instance.memory_mb.unwrap_or(settings.memory_mb),
        extra_jvm_args: &settings.extra_jvm_args,
    };
    launch::spawn(&ctx)
}
