use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub backup_dir: PathBuf,
    pub images_dir: PathBuf,
    pub host: String,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let data_dir = root.join("data");
        let host = std::env::var("SB_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let port = std::env::var("SB_SERVER_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8811);
        Self {
            db_path: data_dir.join("streaming_buddy.db"),
            backup_dir: data_dir.join("backups"),
            images_dir: data_dir.join("images"),
            root,
            data_dir,
            host,
            port,
        }
    }

    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.backup_dir)?;
        std::fs::create_dir_all(&self.images_dir)?;
        Ok(())
    }

    /// Master key source for encrypting settings secrets.
    /// Prefer env var STREAMING_BUDDY_MASTER_KEY; fallback: data/master.key file
    /// (generated once). This keeps secrets encrypted at rest in files/DB.
    pub fn master_key_material(&self) -> anyhow::Result<Vec<u8>> {
        if let Ok(v) = std::env::var("STREAMING_BUDDY_MASTER_KEY") {
            if !v.trim().is_empty() {
                return Ok(v.into_bytes());
            }
        }
        let key_path = self.data_dir.join("master.key");
        if key_path.exists() {
            return Ok(std::fs::read(&key_path)?);
        }
        use rand::RngCore;
        let mut buf = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        // write as base64 text for easy inspection/replacement
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(&key_path, b64.as_bytes())?;
        Ok(buf)
    }
}
