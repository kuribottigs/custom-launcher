//! Persistent account storage (`accounts.json`).
//!
//! Note: tokens are stored in plain text in the launcher data directory,
//! like most third-party launchers. Keep the directory private.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Account;
use crate::config::{write_json_atomic, Paths};
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountStore {
    pub accounts: Vec<Account>,
    /// UUID of the account used for launching.
    pub active: Option<Uuid>,
}

impl AccountStore {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.accounts_file();
        if !file.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(file)?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json_atomic(&paths.accounts_file(), self)
    }

    /// Insert or replace an account (matched by UUID) and make it active.
    pub fn upsert(&mut self, account: Account) {
        self.active = Some(account.uuid);
        if let Some(existing) = self.accounts.iter_mut().find(|a| a.uuid == account.uuid) {
            *existing = account;
        } else {
            self.accounts.push(account);
        }
    }

    pub fn remove(&mut self, uuid: Uuid) {
        self.accounts.retain(|a| a.uuid != uuid);
        if self.active == Some(uuid) {
            self.active = self.accounts.first().map(|a| a.uuid);
        }
    }

    pub fn active_account(&self) -> Option<&Account> {
        self.active
            .and_then(|id| self.accounts.iter().find(|a| a.uuid == id))
            .or_else(|| self.accounts.first())
    }
}
