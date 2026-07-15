//! Building the Java command line and spawning the game process.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::auth::{Account, AccountKind};
use crate::config::Paths;
use crate::{Error, Result};

use super::model::{Argument, VersionJson};
use super::rules::rules_allow;

pub struct LaunchContext<'a> {
    pub paths: &'a Paths,
    pub version: &'a VersionJson,
    /// Version id owning the client jar / natives dir (the vanilla id for
    /// loader profiles).
    pub jar_id: &'a str,
    pub game_dir: &'a Path,
    pub classpath: &'a [PathBuf],
    pub account: &'a Account,
    pub java: &'a Path,
    pub memory_mb: u32,
    pub extra_jvm_args: &'a [String],
}

fn classpath_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}

fn substitute(input: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = input.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    out
}

/// Build the full command line: `[java, jvm-args..., main-class, game-args...]`.
pub fn build_command(ctx: &LaunchContext) -> Result<Vec<String>> {
    let version = ctx.version;
    let main_class = version
        .main_class
        .as_deref()
        .ok_or_else(|| Error::Other(format!("version {} has no mainClass", version.id)))?;

    let mut classpath_entries: Vec<String> = ctx
        .classpath
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    classpath_entries.push(ctx.paths.client_jar(ctx.jar_id).display().to_string());
    let classpath = classpath_entries.join(classpath_separator());

    let natives_dir = ctx.paths.natives_dir(ctx.jar_id);
    let assets_root = ctx.paths.assets_dir();
    let asset_index_id = version
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| version.assets.clone())
        .unwrap_or_else(|| "legacy".to_string());

    // Legacy versions expect assets in a plain directory tree.
    let legacy_assets = assets_root.join("virtual").join(&asset_index_id);
    let game_assets = if legacy_assets.exists() {
        legacy_assets.display().to_string()
    } else {
        assets_root.display().to_string()
    };

    let access_token = ctx
        .account
        .access_token
        .clone()
        .unwrap_or_else(|| "0".to_string());
    let user_type = match ctx.account.kind {
        AccountKind::Microsoft => "msa",
        AccountKind::Offline => "legacy",
    };

    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", ctx.account.username.clone()),
        ("version_name", version.id.clone()),
        ("game_directory", ctx.game_dir.display().to_string()),
        ("assets_root", assets_root.display().to_string()),
        ("game_assets", game_assets),
        ("assets_index_name", asset_index_id),
        ("auth_uuid", ctx.account.uuid.simple().to_string()),
        ("auth_access_token", access_token.clone()),
        ("auth_session", access_token),
        ("auth_xuid", ctx.account.xuid.clone().unwrap_or_default()),
        ("clientid", String::new()),
        ("user_type", user_type.to_string()),
        ("user_properties", "{}".to_string()),
        (
            "version_type",
            version.kind.clone().unwrap_or_else(|| "release".to_string()),
        ),
        ("natives_directory", natives_dir.display().to_string()),
        ("launcher_name", "oxide-launcher".to_string()),
        ("launcher_version", env!("CARGO_PKG_VERSION").to_string()),
        ("classpath", classpath.clone()),
        ("library_directory", ctx.paths.libraries_dir().display().to_string()),
        ("classpath_separator", classpath_separator().to_string()),
        ("resolution_width", "854".to_string()),
        ("resolution_height", "480".to_string()),
    ]);

    let features: HashSet<String> = HashSet::new();
    let expand = |args: &[Argument], out: &mut Vec<String>| {
        for arg in args {
            match arg {
                Argument::Plain(s) => out.push(substitute(s, &vars)),
                Argument::Conditional { rules, value } => {
                    if rules_allow(Some(rules), &features) {
                        for v in value.as_slice() {
                            out.push(substitute(v, &vars));
                        }
                    }
                }
            }
        }
    };

    let mut cmd = vec![ctx.java.display().to_string()];
    cmd.push(format!("-Xmx{}M", ctx.memory_mb));
    cmd.extend(ctx.extra_jvm_args.iter().cloned());

    if let Some(arguments) = &version.arguments {
        expand(&arguments.jvm, &mut cmd);
    } else {
        // Legacy versions define no JVM arguments.
        cmd.push(format!("-Djava.library.path={}", natives_dir.display()));
        cmd.push("-cp".to_string());
        cmd.push(classpath.clone());
    }

    cmd.push(main_class.to_string());

    if let Some(arguments) = &version.arguments {
        expand(&arguments.game, &mut cmd);
    } else if let Some(legacy) = &version.minecraft_arguments {
        for part in legacy.split_whitespace() {
            cmd.push(substitute(part, &vars));
        }
    }

    Ok(cmd)
}

/// Spawn the game. Stdout/stderr are inherited by default; callers that
/// need to capture logs can adjust the returned `Command` themselves via
/// [`build_command`].
pub fn spawn(ctx: &LaunchContext) -> Result<std::process::Child> {
    let cmd = build_command(ctx)?;
    std::fs::create_dir_all(ctx.game_dir)?;
    let child = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(ctx.game_dir)
        .spawn()?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_placeholders() {
        let vars = HashMap::from([("auth_player_name", "Steve".to_string())]);
        assert_eq!(substitute("--username=${auth_player_name}", &vars), "--username=Steve");
    }
}
