pub mod microsoft;
pub mod store;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Microsoft,
    Offline,
}

/// A playable account. For Microsoft accounts the tokens are refreshed on
/// demand; offline accounts carry a deterministic UUID derived from the name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub kind: AccountKind,
    /// Minecraft profile UUID.
    pub uuid: Uuid,
    /// In-game player name.
    pub username: String,
    /// Minecraft access token (Microsoft accounts only).
    #[serde(default)]
    pub access_token: Option<String>,
    /// Unix timestamp (seconds) when `access_token` expires.
    #[serde(default)]
    pub token_expires_at: Option<u64>,
    /// Microsoft OAuth refresh token, used to renew the session.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Xbox user hash / xuid, passed to the game as `--xuid`.
    #[serde(default)]
    pub xuid: Option<String>,
}

impl Account {
    /// Create an offline account. The UUID matches vanilla's offline-mode
    /// derivation (`UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`).
    pub fn offline(name: &str) -> Self {
        Self {
            kind: AccountKind::Offline,
            uuid: offline_uuid(name),
            username: name.to_string(),
            access_token: None,
            token_expires_at: None,
            refresh_token: None,
            xuid: None,
        }
    }

    pub fn token_valid(&self) -> bool {
        match (self.access_token.as_ref(), self.token_expires_at) {
            (Some(_), Some(exp)) => now_unix() + 60 < exp,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Java's `UUID.nameUUIDFromBytes` over `OfflinePlayer:<name>` (MD5-based,
/// version 3, RFC 4122 variant).
pub fn offline_uuid(name: &str) -> Uuid {
    let digest = md5::compute(format!("OfflinePlayer:{name}"));
    let mut bytes = digest.0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_vanilla() {
        // Value produced by Java's UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes(UTF_8))
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }
}
