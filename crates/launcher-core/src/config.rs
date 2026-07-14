use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

/// Filesystem layout of the launcher data directory.
///
/// ```text
/// <root>/
///   settings.json
///   accounts.json
///   meta/                  cached version manifests / version JSONs
///   versions/<id>/<id>.jar client jars
///   libraries/             maven-layout shared libraries
///   assets/                shared assets (indexes/ + objects/)
///   natives/<id>/          extracted native libraries
///   instances/<name>/      per-instance data (instance.json + .minecraft/)
/// ```
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// Default data directory, overridable with the `OXIDE_LAUNCHER_HOME`
    /// environment variable.
    pub fn default_root() -> PathBuf {
        if let Ok(dir) = std::env::var("OXIDE_LAUNCHER_HOME") {
            return PathBuf::from(dir);
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oxide-launcher")
    }

    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn from_env() -> Self {
        Self::new(Self::default_root())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    pub fn meta_dir(&self) -> PathBuf {
        self.root.join("meta")
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn client_jar(&self, version_id: &str) -> PathBuf {
        self.versions_dir()
            .join(version_id)
            .join(format!("{version_id}.jar"))
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn natives_dir(&self, version_id: &str) -> PathBuf {
        self.root.join("natives").join(version_id)
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn instance_dir(&self, name: &str) -> PathBuf {
        self.instances_dir().join(name)
    }
}

/// Launcher-wide settings persisted to `settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Azure AD application (client) ID used for Microsoft login.
    /// Falls back to the `MSA_CLIENT_ID` environment variable.
    #[serde(default)]
    pub msa_client_id: Option<String>,
    /// Explicit path to a `java` executable. Auto-detected when unset.
    #[serde(default)]
    pub java_path: Option<String>,
    /// Maximum JVM heap size in MiB.
    #[serde(default = "default_memory")]
    pub memory_mb: u32,
    /// Extra JVM arguments appended to every launch.
    #[serde(default)]
    pub extra_jvm_args: Vec<String>,
}

fn default_memory() -> u32 {
    2048
}

impl Settings {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.settings_file();
        if !file.exists() {
            return Ok(Self {
                memory_mb: default_memory(),
                ..Default::default()
            });
        }
        let data = std::fs::read_to_string(file)?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json_atomic(&paths.settings_file(), self)
    }

    /// The effective Microsoft client ID (settings override, then env var).
    pub fn effective_client_id(&self) -> Option<String> {
        self.msa_client_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| std::env::var("MSA_CLIENT_ID").ok().filter(|s| !s.is_empty()))
    }
}

/// Serialize `value` to `path` atomically (write to a temp file, then rename).
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
