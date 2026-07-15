//! Locating a suitable Java runtime on the system.

use std::path::PathBuf;
use std::process::Command;

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct JavaInstall {
    pub path: PathBuf,
    pub major_version: u32,
}

/// Parse the major version out of `java -version` output, e.g.
/// `openjdk version "21.0.5"` → 21, `java version "1.8.0_392"` → 8.
fn parse_major_version(output: &str) -> Option<u32> {
    let quoted = output.split('"').nth(1)?;
    let mut parts = quoted.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Run `<path> -version` and report the major version.
pub fn probe(path: &PathBuf) -> Option<JavaInstall> {
    let output = Command::new(path).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr).to_string()
        + &String::from_utf8_lossy(&output.stdout);
    Some(JavaInstall {
        path: path.clone(),
        major_version: parse_major_version(&text)?,
    })
}

fn candidates(explicit: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = explicit {
        out.push(PathBuf::from(path));
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        out.push(PathBuf::from(home).join("bin").join(java_binary()));
    }
    out.push(PathBuf::from(java_binary()));
    // Common JVM install roots.
    for root in ["/usr/lib/jvm", "/Library/Java/JavaVirtualMachines"] {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let base = entry.path();
                out.push(base.join("bin").join(java_binary()));
                out.push(base.join("Contents/Home/bin").join(java_binary()));
            }
        }
    }
    out
}

fn java_binary() -> &'static str {
    if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    }
}

/// Find a Java runtime. Prefers an exact major-version match, then any
/// newer runtime, then anything at all (with the risk the game rejects it).
pub fn find_java(explicit: Option<&str>, required_major: Option<u32>) -> Result<JavaInstall> {
    // An explicitly configured path is trusted as-is if probeable.
    if let Some(path) = explicit {
        if let Some(install) = probe(&PathBuf::from(path)) {
            return Ok(install);
        }
    }
    let mut found: Vec<JavaInstall> = candidates(None)
        .iter()
        .filter_map(probe)
        .collect();
    found.sort_by_key(|j| j.major_version);
    let Some(required) = required_major else {
        return found
            .pop()
            .ok_or(Error::JavaNotFound(0));
    };
    if let Some(exact) = found.iter().find(|j| j.major_version == required) {
        return Ok(exact.clone());
    }
    if let Some(newer) = found.iter().find(|j| j.major_version > required) {
        return Ok(newer.clone());
    }
    found.pop().ok_or(Error::JavaNotFound(required))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(
            parse_major_version("openjdk version \"21.0.5\" 2024-10-15"),
            Some(21)
        );
        assert_eq!(
            parse_major_version("java version \"1.8.0_392\""),
            Some(8)
        );
        assert_eq!(parse_major_version("openjdk version \"17\" 2021"), Some(17));
    }
}
