//! Key-value settings storage (redb).

use crate::error::{Result, SettingsError};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Table definition for settings: key -> value (both strings).
const SETTINGS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("settings");

/// Default key for worker log mode setting.
pub const WORKER_LOG_MODE_KEY: &str = "worker_log_mode";
const PROMPT_CAPTURE_PREFIX: &str = "prompt_capture:";
/// Canonical `adapter:target` string for the instance's home channel.
pub const HOME_CHANNEL_KEY: &str = "home_channel";
/// Whether the stored home channel was set deliberately.
const HOME_CHANNEL_EXPLICIT_KEY: &str = "home_channel_explicit";
/// Whether the agent is holding off on starting new work.
const PAUSED_KEY: &str = "paused";
/// Operator-supplied reason shown wherever the pause surfaces.
const PAUSE_REASON_KEY: &str = "pause_reason";

/// How worker execution logs are stored.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLogMode {
    /// Only log failed worker runs (default).
    #[default]
    ErrorsOnly,
    /// Log all runs with separate directories for success/failure.
    AllSeparate,
    /// Log all runs to the same directory.
    AllCombined,
}

impl std::fmt::Display for WorkerLogMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ErrorsOnly => write!(f, "errors_only"),
            Self::AllSeparate => write!(f, "all_separate"),
            Self::AllCombined => write!(f, "all_combined"),
        }
    }
}

impl std::str::FromStr for WorkerLogMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "errors_only" => Ok(Self::ErrorsOnly),
            "all_separate" => Ok(Self::AllSeparate),
            "all_combined" => Ok(Self::AllCombined),
            _ => Err(format!("unknown worker log mode: {}", s)),
        }
    }
}

/// The instance's default outbound destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeChannel {
    /// Canonical `adapter:target` string, as produced by `BroadcastTarget`.
    pub target: String,
    /// Set deliberately by a principal, rather than adopted on first run.
    pub explicit: bool,
}

/// Settings store backed by redb.
pub struct SettingsStore {
    db: Arc<Database>,
}

impl SettingsStore {
    /// Create a new settings store at the given path.
    /// The database will be created if it doesn't exist.
    pub fn new(path: &Path) -> Result<Self> {
        let db = Database::create(path)
            .map_err(|e| SettingsError::Other(format!("failed to open settings db: {e}")))?;

        // Initialize the table if it doesn't exist
        let write_txn = db
            .begin_write()
            .map_err(|e| SettingsError::Other(format!("failed to begin write txn: {e}")))?;
        {
            let _ = write_txn
                .open_table(SETTINGS_TABLE)
                .map_err(|e| SettingsError::Other(format!("failed to open settings table: {e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| SettingsError::Other(format!("failed to commit write txn: {e}")))?;

        let store = Self { db: Arc::new(db) };

        // Set default values if not present
        if store.get_raw(WORKER_LOG_MODE_KEY).is_err() {
            store.set_raw(WORKER_LOG_MODE_KEY, &WorkerLogMode::default().to_string())?;
        }

        Ok(store)
    }

    /// Get a raw string value by key.
    fn get_raw(&self, key: &str) -> Result<String> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| SettingsError::ReadFailed {
                key: key.to_string(),
                details: e.to_string(),
            })?;

        let table = read_txn
            .open_table(SETTINGS_TABLE)
            .map_err(|e| SettingsError::ReadFailed {
                key: key.to_string(),
                details: e.to_string(),
            })?;

        let value = table
            .get(key)
            .map_err(|e| SettingsError::ReadFailed {
                key: key.to_string(),
                details: e.to_string(),
            })?
            .ok_or_else(|| SettingsError::NotFound {
                key: key.to_string(),
            })?;

        Ok(value.value().to_string())
    }

    /// Read a key, distinguishing "no such key" from a failed read. Callers
    /// that fold both into a default lose the difference between an unset
    /// value and an unreadable store.
    fn get_optional(&self, key: &str) -> Result<Option<String>> {
        match self.get_raw(key) {
            Ok(value) => Ok(Some(value)),
            Err(crate::error::Error::Settings(boxed))
                if matches!(*boxed, SettingsError::NotFound { .. }) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Write several keys in one transaction, so fields that are only
    /// meaningful together cannot be left half-updated by a crash between
    /// two commits.
    fn set_many(&self, pairs: &[(&str, &str)]) -> Result<()> {
        let failed = |key: &str, e: &dyn std::fmt::Display| SettingsError::WriteFailed {
            key: key.to_string(),
            details: e.to_string(),
        };
        let first = pairs.first().map(|(key, _)| *key).unwrap_or_default();

        let write_txn = self.db.begin_write().map_err(|e| failed(first, &e))?;
        {
            let mut table = write_txn
                .open_table(SETTINGS_TABLE)
                .map_err(|e| failed(first, &e))?;
            for (key, value) in pairs {
                table.insert(*key, *value).map_err(|e| failed(key, &e))?;
            }
        }
        write_txn.commit().map_err(|e| failed(first, &e))?;
        Ok(())
    }

    /// Write `pairs` only while `guard_key` holds no non-empty value,
    /// returning whether the write happened.
    ///
    /// The check and the writes share one write transaction, which redb
    /// serializes — two callers racing to claim the same slot cannot both
    /// see it empty.
    fn set_many_if_unset(&self, guard_key: &str, pairs: &[(&str, &str)]) -> Result<bool> {
        let failed = |key: &str, e: &dyn std::fmt::Display| SettingsError::WriteFailed {
            key: key.to_string(),
            details: e.to_string(),
        };

        let write_txn = self.db.begin_write().map_err(|e| failed(guard_key, &e))?;
        let claimed = {
            let mut table = write_txn
                .open_table(SETTINGS_TABLE)
                .map_err(|e| failed(guard_key, &e))?;
            let taken = table
                .get(guard_key)
                .map_err(|e| failed(guard_key, &e))?
                .is_some_and(|value| !value.value().is_empty());
            if taken {
                false
            } else {
                for (key, value) in pairs {
                    table.insert(*key, *value).map_err(|e| failed(key, &e))?;
                }
                true
            }
        };
        write_txn.commit().map_err(|e| failed(guard_key, &e))?;
        Ok(claimed)
    }

    /// Set a raw string value by key.
    fn set_raw(&self, key: &str, value: &str) -> Result<()> {
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| SettingsError::WriteFailed {
                key: key.to_string(),
                details: e.to_string(),
            })?;

        {
            let mut table =
                write_txn
                    .open_table(SETTINGS_TABLE)
                    .map_err(|e| SettingsError::WriteFailed {
                        key: key.to_string(),
                        details: e.to_string(),
                    })?;

            table
                .insert(key, value)
                .map_err(|e| SettingsError::WriteFailed {
                    key: key.to_string(),
                    details: e.to_string(),
                })?;
        }

        write_txn.commit().map_err(|e| SettingsError::WriteFailed {
            key: key.to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Get the worker log mode setting.
    pub fn worker_log_mode(&self) -> WorkerLogMode {
        match self.get_raw(WORKER_LOG_MODE_KEY) {
            Ok(raw) => raw.parse().unwrap_or_default(),
            Err(_) => WorkerLogMode::default(),
        }
    }

    /// Set the worker log mode setting.
    pub fn set_worker_log_mode(&self, mode: WorkerLogMode) -> Result<()> {
        self.set_raw(WORKER_LOG_MODE_KEY, &mode.to_string())
    }

    /// Check whether prompt capture is enabled for a specific channel.
    pub fn prompt_capture_enabled(&self, channel_id: &str) -> bool {
        let key = format!("{PROMPT_CAPTURE_PREFIX}{channel_id}");
        matches!(self.get_raw(&key), Ok(v) if v == "true")
    }

    /// Enable or disable prompt capture for a specific channel.
    pub fn set_prompt_capture(&self, channel_id: &str, enabled: bool) -> Result<()> {
        let key = format!("{PROMPT_CAPTURE_PREFIX}{channel_id}");
        self.set_raw(&key, if enabled { "true" } else { "false" })
    }

    /// The instance's home channel, or `None` when unset.
    ///
    /// An unreadable store reports "no home", which costs a proactive message
    /// its destination — the message is recorded instead. Adoption does not
    /// build on this read, so a failure here cannot displace a stored home.
    pub fn home_channel(&self) -> Option<HomeChannel> {
        let target = match self.get_optional(HOME_CHANNEL_KEY) {
            Ok(target) => target.filter(|t| !t.is_empty())?,
            Err(error) => {
                tracing::warn!(%error, "failed to read home channel; treating as unset");
                return None;
            }
        };
        let explicit = match self.get_optional(HOME_CHANNEL_EXPLICIT_KEY) {
            Ok(value) => value.as_deref() == Some("true"),
            Err(error) => {
                tracing::warn!(%error, "failed to read home channel provenance");
                false
            }
        };
        Some(HomeChannel { target, explicit })
    }

    /// Set the home channel deliberately, replacing whatever was there.
    pub fn set_home_channel(&self, target: &str) -> Result<()> {
        self.set_many(&[
            (HOME_CHANNEL_KEY, target),
            (HOME_CHANNEL_EXPLICIT_KEY, "true"),
        ])
    }

    /// Why the agent is paused, or `None` when it is running normally. A
    /// pause with no stated reason yields an empty string.
    /// An unreadable store reports paused. A stop the operator asked for
    /// outranks the agent's availability: resuming is one command away, but
    /// silently running through an emergency stop is not recoverable.
    pub fn pause_reason(&self) -> Option<String> {
        match self.get_optional(PAUSED_KEY) {
            Ok(value) => {
                if value.as_deref() != Some("true") {
                    return None;
                }
            }
            Err(error) => {
                tracing::error!(%error, "failed to read pause state; holding work until it reads");
                return Some("pause state unreadable".to_string());
            }
        }
        Some(
            self.get_optional(PAUSE_REASON_KEY)
                .unwrap_or_default()
                .unwrap_or_default(),
        )
    }

    /// Hold off on starting new work, or resume. Survives restart so an
    /// emergency stop is not undone by a bounce.
    pub fn set_paused(&self, reason: Option<&str>) -> Result<()> {
        match reason {
            Some(reason) => self.set_many(&[(PAUSE_REASON_KEY, reason), (PAUSED_KEY, "true")]),
            None => self.set_many(&[(PAUSE_REASON_KEY, ""), (PAUSED_KEY, "false")]),
        }
    }

    /// Drop the home channel, returning the instance to sending nothing on
    /// its own.
    pub fn clear_home_channel(&self) -> Result<()> {
        self.set_many(&[(HOME_CHANNEL_KEY, ""), (HOME_CHANNEL_EXPLICIT_KEY, "false")])
    }

    /// Adopt `target` as the home channel only when nothing has claimed it.
    /// Returns whether the value was taken. An implicit home never overwrites
    /// an explicit one, and never overwrites another implicit one — first run
    /// wins until a principal sets it deliberately.
    pub fn adopt_home_channel(&self, target: &str) -> Result<bool> {
        self.set_many_if_unset(
            HOME_CHANNEL_KEY,
            &[
                (HOME_CHANNEL_KEY, target),
                (HOME_CHANNEL_EXPLICIT_KEY, "false"),
            ],
        )
    }
}

impl std::fmt::Debug for SettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsStore").finish_non_exhaustive()
    }
}
