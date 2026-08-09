//! Daemon lifecycle control: coordinated shutdown and self-restart.
//!
//! The daemon's shutdown signal is a `watch` channel carrying a
//! [`LifecycleState`]. A [`LifecycleHandle`] is the only writer: IPC, the HTTP
//! API, and the restart tool all request transitions through it. Requests are
//! armed with a grace delay so in-flight HTTP responses and tool results can
//! flush before teardown begins.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Delay between a lifecycle request and the shutdown signal firing, giving
/// in-flight HTTP responses and tool results time to flush.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// What the daemon should do once the main loop observes a non-`Running` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LifecycleState {
    #[default]
    Running,
    /// Tear down and exit the process.
    Exit,
    /// Tear down and re-exec the same binary in place.
    Restart,
}

const INTENT_NONE: u8 = 0;
const INTENT_RESTART: u8 = 1;
const INTENT_EXIT: u8 = 2;

/// Requests daemon shutdown or restart. Cheap to clone; all clones share the
/// same underlying channel and intent guard.
#[derive(Clone)]
pub struct LifecycleHandle {
    tx: watch::Sender<LifecycleState>,
    intent: Arc<AtomicU8>,
}

impl std::fmt::Debug for LifecycleHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleHandle")
            .field("intent", &self.intent.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl LifecycleHandle {
    pub fn new() -> (Self, watch::Receiver<LifecycleState>) {
        let (tx, rx) = watch::channel(LifecycleState::Running);
        let handle = Self {
            tx,
            intent: Arc::new(AtomicU8::new(INTENT_NONE)),
        };
        (handle, rx)
    }

    pub fn subscribe(&self) -> watch::Receiver<LifecycleState> {
        self.tx.subscribe()
    }

    /// Request a restart. Returns `false` when a restart is already pending or
    /// a shutdown has been requested (shutdown always wins over restart).
    pub fn request_restart(&self, reason: &str) -> bool {
        let claimed = self
            .intent
            .compare_exchange(
                INTENT_NONE,
                INTENT_RESTART,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok();
        if !claimed {
            return false;
        }
        tracing::info!(reason, "restart requested");
        self.arm();
        true
    }

    /// Request a shutdown. Upgrades a pending restart to a plain exit. Returns
    /// `false` when a shutdown is already pending.
    pub fn request_shutdown(&self, reason: &str) -> bool {
        match self.intent.swap(INTENT_EXIT, Ordering::SeqCst) {
            INTENT_EXIT => false,
            INTENT_NONE => {
                tracing::info!(reason, "shutdown requested");
                self.arm();
                true
            }
            _ => {
                // A restart was already armed; the delayed sender reads the
                // final intent at fire time and will emit `Exit` instead.
                tracing::info!(reason, "shutdown requested, superseding pending restart");
                true
            }
        }
    }

    /// Spawn the single delayed sender. The state to emit is read at fire time
    /// so a shutdown arriving during the grace window supersedes a restart.
    fn arm(&self) {
        let tx = self.tx.clone();
        let intent = Arc::clone(&self.intent);
        tokio::spawn(async move {
            tokio::time::sleep(SHUTDOWN_GRACE).await;
            let state = match intent.load(Ordering::SeqCst) {
                INTENT_EXIT => LifecycleState::Exit,
                INTENT_RESTART => LifecycleState::Restart,
                _ => return,
            };
            let _ = tx.send(state);
        });
    }
}

/// Everything needed to re-exec the daemon in place after teardown.
///
/// Captured at process start so a restart re-runs the daemon exactly as it was
/// launched. The stored exe path deliberately re-resolves at exec time: a
/// binary replaced in place (native self-update) is picked up by the restart.
#[derive(Debug, Clone)]
pub struct RestartSpec {
    pub exe: PathBuf,
    pub args: Vec<std::ffi::OsString>,
}

impl RestartSpec {
    /// Capture before daemonizing, while the working directory is still the
    /// caller's — the daemon's cwd is `/` by the time the spec is used, so a
    /// relative `--config` must be canonicalized here.
    pub fn capture(config_path: Option<&Path>, debug: bool, foreground: bool) -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| {
            std::env::args_os()
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("spacebot"))
        });
        Self {
            exe,
            args: Self::build_args(config_path, debug, foreground),
        }
    }

    fn build_args(
        config_path: Option<&Path>,
        debug: bool,
        foreground: bool,
    ) -> Vec<std::ffi::OsString> {
        let mut args = Vec::new();
        if let Some(path) = config_path {
            let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            args.push("--config".into());
            args.push(canonical.into_os_string());
        }
        if debug {
            args.push("--debug".into());
        }
        args.push("start".into());
        if foreground {
            args.push("--foreground".into());
        }
        args
    }

    /// Replace the current process image. Only returns on failure.
    ///
    /// `SPACEBOT_REEXEC` tells the re-exec'd startup path that stdin is no
    /// longer a terminal, so interactive onboarding must not run.
    pub fn exec(&self) -> std::io::Error {
        use std::os::unix::process::CommandExt as _;
        std::process::Command::new(&self.exe)
            .args(&self.args)
            .env("SPACEBOT_REEXEC", "1")
            .exec()
    }
}

/// Restart context persisted across the re-exec so the daemon can announce
/// itself back in the channel that asked for the reboot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRestart {
    pub agent_id: String,
    pub conversation_id: String,
    /// Messaging adapter name for the comeback note; `None` means restart
    /// silently (no channel context, e.g. API- or worker-triggered).
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    pub reason: String,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub source: String,
}

impl PendingRestart {
    pub fn path(instance_dir: &Path) -> PathBuf {
        instance_dir.join("pending_restart.json")
    }

    /// Persist the marker atomically (temp file + rename) so a crash mid-write
    /// can never leave a truncated marker for the next boot to trip on.
    pub fn write(&self, instance_dir: &Path) -> anyhow::Result<()> {
        let tmp = instance_dir.join("pending_restart.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, Self::path(instance_dir))?;
        Ok(())
    }

    /// Read and remove the marker. The file is deleted even when it fails to
    /// parse so a bad marker can't replay on every boot.
    pub fn take(instance_dir: &Path) -> Option<Self> {
        let path = Self::path(instance_dir);
        let content = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        match serde_json::from_str(&content) {
            Ok(pending) => Some(pending),
            Err(error) => {
                tracing::warn!(%error, "discarding unreadable pending_restart.json");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn restart_fires_after_grace_delay() {
        let (handle, mut rx) = LifecycleHandle::new();
        assert!(handle.request_restart("test"));
        rx.wait_for(|state| *state != LifecycleState::Running)
            .await
            .unwrap();
        assert_eq!(*rx.borrow(), LifecycleState::Restart);
    }

    #[tokio::test(start_paused = true)]
    async fn second_restart_request_is_rejected() {
        let (handle, _rx) = LifecycleHandle::new();
        assert!(handle.request_restart("first"));
        assert!(!handle.request_restart("second"));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_supersedes_pending_restart() {
        let (handle, mut rx) = LifecycleHandle::new();
        assert!(handle.request_restart("restart"));
        assert!(handle.request_shutdown("stop"));
        rx.wait_for(|state| *state != LifecycleState::Running)
            .await
            .unwrap();
        assert_eq!(*rx.borrow(), LifecycleState::Exit);
    }

    #[tokio::test(start_paused = true)]
    async fn restart_after_shutdown_is_rejected() {
        let (handle, _rx) = LifecycleHandle::new();
        assert!(handle.request_shutdown("stop"));
        assert!(!handle.request_restart("too late"));
        assert!(!handle.request_shutdown("again"));
    }

    #[test]
    fn restart_spec_args_minimal() {
        let spec = RestartSpec::capture(None, false, false);
        assert_eq!(spec.args, vec![std::ffi::OsString::from("start")]);
    }

    #[test]
    fn restart_spec_args_full_flags() {
        let spec = RestartSpec::capture(None, true, true);
        let expected: Vec<std::ffi::OsString> =
            vec!["--debug".into(), "start".into(), "--foreground".into()];
        assert_eq!(spec.args, expected);
    }

    #[test]
    fn restart_spec_canonicalizes_config_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "").unwrap();

        let spec = RestartSpec::capture(Some(&config), false, false);
        assert_eq!(spec.args[0], std::ffi::OsString::from("--config"));
        let stored = PathBuf::from(spec.args[1].clone());
        assert!(stored.is_absolute());
        assert_eq!(stored, config.canonicalize().unwrap());
        assert_eq!(spec.args[2], std::ffi::OsString::from("start"));
    }

    #[test]
    fn pending_restart_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let pending = PendingRestart {
            agent_id: "main".into(),
            conversation_id: "discord:123".into(),
            adapter: Some("discord".into()),
            target: Some("123".into()),
            reason: "config change".into(),
            requested_at: chrono::Utc::now(),
            source: "tool".into(),
        };
        pending.write(dir.path()).unwrap();

        let taken = PendingRestart::take(dir.path()).unwrap();
        assert_eq!(taken.conversation_id, "discord:123");
        assert_eq!(taken.adapter.as_deref(), Some("discord"));
        assert!(!PendingRestart::path(dir.path()).exists());
        assert!(PendingRestart::take(dir.path()).is_none());
    }

    #[test]
    fn unparseable_marker_is_discarded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(PendingRestart::path(dir.path()), "not json").unwrap();
        assert!(PendingRestart::take(dir.path()).is_none());
        assert!(!PendingRestart::path(dir.path()).exists());
    }
}
