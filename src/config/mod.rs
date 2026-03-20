use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Clone)]
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
}

#[derive(Deserialize, Clone)]
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
        let content = format!(
            r#"shell = {:?}
font_family = {:?}
font_size = {}
scrollback_limit = {}
columns = {}
rows = {}
opacity = {}
fg_color = {:?}
bg_color = {:?}

[hotkey]
enabled = {}
modifiers = {:?}
key = {:?}
"#,
            self.shell, self.font_family, self.font_size,
            self.scrollback_limit, self.columns, self.rows,
            self.opacity, self.fg_color, self.bg_color,
            self.hotkey.enabled,
            self.hotkey.modifiers, self.hotkey.key,
        );
        let path = config_path();
        std::fs::write(path, content)
    }
}

fn config_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("config.toml")))
        .unwrap_or_else(|| std::path::PathBuf::from("config.toml"))
}

impl Config {
    pub fn load() -> Self {
        let paths = [
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("config.toml"))),
            Some(PathBuf::from("config.toml")),
            dirs_config().map(|d| d.join("config.toml")),
        ];

        for path in paths.iter().flatten() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(config) = toml::from_str::<Config>(&content) {
                    return config;
                }
            }
        }

        Config::default()
    }
}

fn dirs_config() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|d| PathBuf::from(d).join("terminal"))
}
