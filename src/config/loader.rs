use serde::Deserialize;
use std::{env, net::SocketAddr, path::{Path, PathBuf}};

/// Instruction queue settings, read from the `instruction_queue` object of the JSON config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QueueConfig {
    /// Look-ahead window (seconds) measured from the enqueue time of the instruction being
    /// consumed; same-target pending sends enqueued inside it are merged into one dispatch.
    pub merge_window_secs: u64,
    /// Delay (ms) between consecutive messages of a merged dispatch (two or more requests).
    /// An unmerged request keeps its own `interval_ms`.
    pub merged_interval_ms: u64,
    /// Upper bound of pending records; further submissions are rejected before enqueue.
    pub max_pending: u32,
    /// Terminal records (payload already purged) older than this are deleted.
    pub retention_secs: u64,
}

pub const DATABASE_FILE: &str = "desk_agent.sqlite3";
const LEGACY_DATABASE_FILE: &str = "security.sqlite3";

pub const MAX_MERGE_WINDOW_SECS: u64 = 86_400;
pub const MAX_PENDING_LIMIT: u32 = 10_000;

impl Default for QueueConfig {
    fn default() -> Self {
        Self { merge_window_secs: 300, merged_interval_ms: 1_000, max_pending: 1_000, retention_secs: 7 * 86_400 }
    }
}

impl QueueConfig {
    fn validate(self) -> anyhow::Result<Self> {
        anyhow::ensure!(self.merge_window_secs <= MAX_MERGE_WINDOW_SECS, "instruction_queue.merge_window_secs must be 0..=86400");
        anyhow::ensure!(self.merged_interval_ms <= 60_000, "instruction_queue.merged_interval_ms must be 0..=60000");
        anyhow::ensure!((1..=MAX_PENDING_LIMIT).contains(&self.max_pending), "instruction_queue.max_pending must be 1..=10000");
        anyhow::ensure!(self.retention_secs >= 60, "instruction_queue.retention_secs must be at least 60");
        Ok(self)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    instruction_queue: QueueConfig,
}

/// `DESK_AGENT_CONFIG` or `~/.desk-agent/config.json`. A missing default file yields defaults;
/// an explicitly configured path must exist. Invalid content is an error, never silently ignored.
pub fn config_path() -> Option<PathBuf> {
    env::var_os("DESK_AGENT_CONFIG").map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".desk-agent").join("config.json")))
}

pub fn load_queue_config(path: Option<&Path>, required: bool) -> anyhow::Result<QueueConfig> {
    let Some(path) = path else { return Ok(QueueConfig::default()) };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => return Ok(QueueConfig::default()),
        Err(error) => return Err(anyhow::anyhow!("cannot read config file {}: {error}", path.display())),
    };
    let file: FileConfig = serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("invalid config file {}: {error}", path.display()))?;
    file.instruction_queue.validate()
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub bind: SocketAddr,
    /// The single private project database (all tables): `DESK_AGENT_DB` or
    /// `~/.desk-agent/desk_agent.sqlite3`.
    pub database_path: PathBuf,
    /// Pre-consolidation private database renamed to `database_path` once when the latter is
    /// missing; only considered for the default location.
    pub legacy_database_path: Option<PathBuf>,
    pub socket_path: PathBuf,
    pub queue: QueueConfig,
}

impl RuntimeConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind: SocketAddr = env::var("DESK_AGENT_BIND")
            .unwrap_or_else(|_| "127.0.0.1:17890".into())
            .parse()?;
        anyhow::ensure!(
            bind.ip().is_loopback(),
            "DESK_AGENT_BIND must be a loopback address"
        );

        let home_dir = env::var_os("HOME").map(|home| PathBuf::from(home).join(".desk-agent"));
        let (database_path, legacy_database_path) = match env::var_os("DESK_AGENT_DB") {
            Some(path) => (PathBuf::from(path), None),
            None => (
                home_dir.as_ref().map(|dir| dir.join(DATABASE_FILE)).unwrap_or_default(),
                home_dir.as_ref().map(|dir| dir.join(LEGACY_DATABASE_FILE)),
            ),
        };
        let socket_path = env::var_os("DESK_AGENT_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let user = env::var("USER").unwrap_or_else(|_| "local".to_owned());
                env::temp_dir().join(format!("desk-agent-{user}.sock"))
            });
        let queue = load_queue_config(config_path().as_deref(), env::var_os("DESK_AGENT_CONFIG").is_some())?;
        Ok(Self {
            queue,
            bind,
            database_path,
            legacy_database_path,
            socket_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, text: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("desk-agent-config-{}-{name}.json", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn queue_config_defaults_and_overrides() {
        assert_eq!(load_queue_config(None, false).unwrap(), QueueConfig::default());
        let missing = env::temp_dir().join("desk-agent-config-missing-never.json");
        assert_eq!(load_queue_config(Some(&missing), false).unwrap(), QueueConfig::default());
        assert!(load_queue_config(Some(&missing), true).is_err());
        let path = write("ok", r#"{"instruction_queue":{"merge_window_secs":60,"merged_interval_ms":0}}"#);
        let config = load_queue_config(Some(&path), true).unwrap();
        assert_eq!(config.merge_window_secs, 60);
        assert_eq!(config.merged_interval_ms, 0);
        assert_eq!(config.max_pending, QueueConfig::default().max_pending);
        for (name, text) in [
            ("unknown", r#"{"instruction_queue":{"merge_window":60}}"#),
            ("range", r#"{"instruction_queue":{"merge_window_secs":86401}}"#),
            ("pending", r#"{"instruction_queue":{"max_pending":0}}"#),
            ("interval", r#"{"instruction_queue":{"merged_interval_ms":60001}}"#),
            ("syntax", "{"),
        ] {
            let path = write(name, text);
            assert!(load_queue_config(Some(&path), true).is_err(), "{name}");
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(path);
    }
}
