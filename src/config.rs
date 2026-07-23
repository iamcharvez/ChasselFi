use serde::{Deserialize, Serialize};
use std::{fs, net::SocketAddr, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: String,
    pub hardware_mode: HardwareMode,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HardwareMode {
    Simulated,
    Linux,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".parse().expect("valid default address"),
            data_dir: "data".into(),
            hardware_mode: HardwareMode::Simulated,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = std::env::var("CHASSELFI_CONFIG")
            .or_else(|_| std::env::var("BANTAY_CONFIG"))
            .unwrap_or_else(|_| "config.json".into());
        if !Path::new(&path).exists() {
            return Self::default();
        }
        fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }
}
