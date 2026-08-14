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
        let mut config = if !Path::new(&path).exists() {
            Self::default()
        } else {
            fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default()
        };

        // The packaged service uses an environment override so upgrades can
        // move installations created by early simulation-only releases into
        // Linux mode without destructively rewriting the operator's JSON.
        if let Ok(mode) = std::env::var("CHASSELFI_HARDWARE_MODE") {
            config.hardware_mode = match mode.trim().to_ascii_lowercase().as_str() {
                "linux" => HardwareMode::Linux,
                "simulated" => HardwareMode::Simulated,
                invalid => panic!(
                    "CHASSELFI_HARDWARE_MODE must be 'linux' or 'simulated', got '{invalid}'"
                ),
            };
        }
        config
    }
}
