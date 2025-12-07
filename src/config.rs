//! Settings persistence

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub history_length: i32,
    pub background_color: String,
    pub current_color: String,
    pub history_color: String,
    pub threads: i32,
    pub limit: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            history_length: 5,
            background_color: "#000000".to_string(),
            current_color: "#00ff00".to_string(),
            history_color: "#ff7f00".to_string(),
            threads: 0,
            limit: 0,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "imsel", "radar_echo_trails")
        .map(|dirs| dirs.config_dir().join("settings.json"))
}

pub fn load_settings() -> Result<Settings, Box<dyn std::error::Error>> {
    let path = settings_path().ok_or("Could not determine config directory")?;
    let content = std::fs::read_to_string(path)?;
    let settings: Settings = serde_json::from_str(&content)?;
    Ok(settings)
}

pub fn save_settings(settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
    let path = settings_path().ok_or("Could not determine config directory")?;
    
    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    let content = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, content)?;
    Ok(())
}
