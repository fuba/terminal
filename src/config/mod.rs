use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct Config {
    pub shell: String,
    pub font_family: String,
    pub font_size: f32,
    pub scrollback_limit: usize,
    pub columns: u32,
    pub rows: u32,
    pub opacity: u8,
    pub fg_color: String,
    pub bg_color: String,
    pub hotkey: HotkeyConfig,
    #[serde(default)]
    pub ssh_profiles: Vec<crate::pty::SshProfile>,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub window_positions: HashMap<String, WindowPosition>,
}

/// A bookmark for quick-launch from the tab bar dropdown.
/// Either a local shell command, or refers to an SSH profile by name.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Bookmark {
    pub name: String,
    /// Local shell command to spawn (mutually exclusive with `ssh`)
    pub shell: Option<String>,
    /// Name of SSH profile to launch (mutually exclusive with `shell`)
    pub ssh: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct WindowPosition {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct HotkeyConfig {
    pub enabled: bool,
    /// "ctrl", "alt", "shift", "win" (comma-separated for multiple)
    pub modifiers: String,
    /// Virtual key name: "grave", "space", "f12", etc.
    pub key: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shell: "powershell.exe".into(),
            font_family: "Consolas".into(),
            font_size: 16.0,
            scrollback_limit: 10000,
            columns: 120,
            rows: 30,
            opacity: 100,
            fg_color: "#CCCCCC".into(),
            bg_color: "#0C0C0C".into(),
            hotkey: HotkeyConfig::default(),
            ssh_profiles: Vec::new(),
            bookmarks: Vec::new(),
            favorites: Vec::new(),
            window_positions: HashMap::new(),
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        HotkeyConfig {
            enabled: true,
            modifiers: "alt,shift".into(),
            key: "v".into(),
        }
    }
}

impl HotkeyConfig {
    pub fn modifier_flags(&self) -> u32 {
        let mut flags = 0u32;
        for m in self.modifiers.split(',') {
            match m.trim().to_lowercase().as_str() {
                "alt" => flags |= 0x0001,
                "ctrl" => flags |= 0x0002,
                "shift" => flags |= 0x0004,
                "win" => flags |= 0x0008,
                _ => {}
            }
        }
        flags
    }

    pub fn virtual_key(&self) -> u32 {
        match self.key.to_lowercase().as_str() {
            "grave" | "oem3" | "`" => 0xC0,
            "space" => 0x20,
            "tab" => 0x09,
            "f1" => 0x70, "f2" => 0x71, "f3" => 0x72, "f4" => 0x73,
            "f5" => 0x74, "f6" => 0x75, "f7" => 0x76, "f8" => 0x77,
            "f9" => 0x78, "f10" => 0x79, "f11" => 0x7A, "f12" => 0x7B,
            s if s.len() == 1 => {
                let c = s.chars().next().unwrap().to_ascii_uppercase();
                if c.is_ascii_alphanumeric() { c as u32 } else { 0 }
            }
            _ => 0,
        }
    }
}

impl Config {
    pub fn parse_color(hex: &str) -> (u8, u8, u8) {
        let hex = hex.trim_start_matches('#');
        if hex.len() >= 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(204);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(204);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(204);
            (r, g, b)
        } else {
            (204, 204, 204)
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let path = config_path();
        let _ = std::fs::create_dir_all(config_dir());
        std::fs::write(path, content)
    }
}

fn config_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("terminal"))
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(config) = toml::from_str::<Config>(&content) {
                return config;
            }
        }
        Config::default()
    }
}
