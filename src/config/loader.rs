use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub bind: SocketAddr,
    pub database_url: String,
    pub socket_path: PathBuf,
    pub security_database_path: PathBuf,
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

        let database_url = match env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                let path = env::var_os("DESK_AGENT_DB")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("desk-agent.sqlite3"));
                format!("sqlite://{}", path.display())
            }
        };
        let security_database_path = env::var_os("DESK_AGENT_SECURITY_DB").map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".desk-agent").join("security.sqlite3")))
            .unwrap_or_default();
        let socket_path = env::var_os("DESK_AGENT_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let user = env::var("USER").unwrap_or_else(|_| "local".to_owned());
                env::temp_dir().join(format!("desk-agent-{user}.sock"))
            });
        Ok(Self {
            bind,
            database_url,
            socket_path,
            security_database_path,
        })
    }
}
