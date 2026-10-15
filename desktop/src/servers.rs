use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use url::Url;

const DEFAULT_SERVER: &str = "https://app.librepaper.org/";

#[derive(Serialize, Deserialize)]
pub struct Servers {
    servers: Vec<String>,
    // An environment override applies to this run only, so it is never saved.
    #[serde(skip)]
    from_env: bool,
}

impl Servers {
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        if let Ok(server) = std::env::var("LIBREPAPER_SERVER") {
            return Ok(Self {
                servers: vec![server],
                from_env: true,
            });
        }
        let path = file(app)?;
        match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self {
                servers: vec![DEFAULT_SERVER.to_string()],
                from_env: false,
            }),
            Err(err) => Err(format!("{}: {err}", path.display())),
        }
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        if self.from_env {
            return Ok(());
        }
        let path = file(app)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|err| err.to_string())?;
        fs::write(&path, bytes).map_err(|err| err.to_string())
    }

    pub fn first(&self) -> Option<Url> {
        self.servers.first().and_then(|raw| accepted(raw))
    }

    pub fn trusted(&self, origin: &str) -> bool {
        self.servers
            .iter()
            .filter_map(|raw| accepted(raw))
            .any(|url| url.origin().ascii_serialization() == origin)
    }

    pub fn add(&mut self, url: &Url) {
        self.servers.push(url.origin().ascii_serialization());
    }
}

// Plain http is only accepted on loopback, so a development server can run
// without certificates. Everything else must be https.
pub fn accepted(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => Some(url),
        "http" if loopback => Some(url),
        _ => None,
    }
}

fn file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|err| err.to_string())?;
    Ok(dir.join("desktop.json"))
}
