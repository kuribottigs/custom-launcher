//! Core library for the Oxide Launcher: Microsoft/offline authentication,
//! Minecraft version management and downloads, Fabric loader support,
//! Modrinth API access, instance management, and game launching.

pub mod auth;
pub mod config;
pub mod error;
pub mod http;
pub mod instance;
pub mod minecraft;
pub mod modrinth;
pub mod progress;

pub use error::{Error, Result};
