use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    #[default]
    Center,
    Top,
    Bottom,
}

pub struct PositionInfo {
    pub id: OverlayPosition,
    pub label: &'static str,
}

pub const POSITIONS: &[PositionInfo] = &[
    PositionInfo {
        id: OverlayPosition::Center,
        label: "Center",
    },
    PositionInfo {
        id: OverlayPosition::Top,
        label: "Top",
    },
    PositionInfo {
        id: OverlayPosition::Bottom,
        label: "Bottom",
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub hotkey: String,
    #[serde(default)]
    pub overlay_position: OverlayPosition,
    #[serde(default = "default_live_transcription")]
    pub live_transcription: bool,
    #[serde(default = "default_sound_effects")]
    pub sound_effects: bool,
    #[serde(default = "default_show_accessibility_helper_on_launch")]
    pub show_accessibility_helper_on_launch: bool,
    #[serde(default)]
    pub accessibility_helper_dismissed: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "base".to_string(),
            hotkey: "left_control".to_string(),
            overlay_position: OverlayPosition::default(),
            live_transcription: default_live_transcription(),
            sound_effects: default_sound_effects(),
            show_accessibility_helper_on_launch: default_show_accessibility_helper_on_launch(),
            accessibility_helper_dismissed: false,
        }
    }
}

fn default_live_transcription() -> bool {
    true
}

fn default_sound_effects() -> bool {
    true
}

fn default_show_accessibility_helper_on_launch() -> bool {
    true
}

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub size: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        label: "Tiny",
        size: "75 MB",
    },
    ModelInfo {
        id: "base",
        label: "Base",
        size: "142 MB",
    },
    ModelInfo {
        id: "small",
        label: "Small",
        size: "466 MB",
    },
];

pub struct HotkeyInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub modifier_flag: u64,
    pub device_flag: u64,
}

pub const HOTKEYS: &[HotkeyInfo] = &[
    HotkeyInfo {
        id: "left_control",
        label: "Left Control ⌃",
        modifier_flag: 0x00040000,
        device_flag: 0x00000001,
    },
    HotkeyInfo {
        id: "right_control",
        label: "Right Control ⌃",
        modifier_flag: 0x00040000,
        device_flag: 0x00002000,
    },
    HotkeyInfo {
        id: "left_option",
        label: "Left Option ⌥",
        modifier_flag: 0x00080000,
        device_flag: 0x00000020,
    },
    HotkeyInfo {
        id: "right_option",
        label: "Right Option ⌥",
        modifier_flag: 0x00080000,
        device_flag: 0x00000040,
    },
    HotkeyInfo {
        id: "left_command",
        label: "Left Command ⌘",
        modifier_flag: 0x00100000,
        device_flag: 0x00000008,
    },
    HotkeyInfo {
        id: "right_command",
        label: "Right Command ⌘",
        modifier_flag: 0x00100000,
        device_flag: 0x00000010,
    },
    HotkeyInfo {
        id: "fn",
        label: "Fn (Globe) 🌐",
        modifier_flag: 0x00800000,
        device_flag: 0,
    },
];

impl Config {
    fn config_dir() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~"));
        base.join("Screamer")
    }

    fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .map(Self::normalized)
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = Self::config_dir();
        if let Err(err) = fs::create_dir_all(&dir) {
            eprintln!("[screamer] Failed to create config directory: {err}");
            return;
        }

        let normalized = self.clone().normalized();
        let mut json = match serde_json::to_vec_pretty(&normalized) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("[screamer] Failed to serialize config: {err}");
                return;
            }
        };
        json.push(b'\n');

        let path = Self::config_path();
        let tmp = path.with_extension("json.tmp");
        let mut file = match File::create(&tmp) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("[screamer] Failed to create temporary config file: {err}");
                return;
            }
        };

        if let Err(err) = file.write_all(&json).and_then(|_| file.sync_all()) {
            eprintln!("[screamer] Failed to write config: {err}");
            let _ = fs::remove_file(&tmp);
            return;
        }

        if let Err(err) = fs::rename(&tmp, &path) {
            eprintln!("[screamer] Failed to replace config file: {err}");
            let _ = fs::remove_file(&tmp);
        }
    }

    pub fn hotkey_info(&self) -> &'static HotkeyInfo {
        HOTKEYS
            .iter()
            .find(|h| h.id == self.hotkey)
            .unwrap_or(&HOTKEYS[0])
    }

    pub fn hotkey_label(&self) -> &'static str {
        self.hotkey_info().label
    }

    pub fn position_label(&self) -> &'static str {
        POSITIONS
            .iter()
            .find(|p| p.id == self.overlay_position)
            .map(|p| p.label)
            .unwrap_or("Center")
    }

    fn normalized(mut self) -> Self {
        let default = Self::default();

        if !MODELS.iter().any(|model| model.id == self.model) {
            self.model = default.model;
        }

        if !HOTKEYS.iter().any(|hotkey| hotkey.id == self.hotkey) {
            self.hotkey = default.hotkey;
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = Config::default();
        assert_eq!(config.model, "base");
        assert_eq!(config.hotkey, "left_control");
        assert_eq!(config.overlay_position, OverlayPosition::Center);
        assert!(config.live_transcription);
        assert!(config.sound_effects);
        assert!(config.show_accessibility_helper_on_launch);
        assert!(!config.accessibility_helper_dismissed);
    }

    #[test]
    fn config_roundtrip() {
        let config = Config {
            model: "tiny".to_string(),
            hotkey: "fn".to_string(),
            overlay_position: OverlayPosition::Bottom,
            live_transcription: false,
            sound_effects: false,
            show_accessibility_helper_on_launch: false,
            accessibility_helper_dismissed: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "tiny");
        assert_eq!(parsed.hotkey, "fn");
        assert_eq!(parsed.overlay_position, OverlayPosition::Bottom);
        assert!(!parsed.live_transcription);
        assert!(!parsed.sound_effects);
        assert!(!parsed.show_accessibility_helper_on_launch);
        assert!(parsed.accessibility_helper_dismissed);
    }

    #[test]
    fn config_backward_compat() {
        // Old config without newer fields should deserialize with defaults.
        let json = r#"{"model":"base","hotkey":"left_control"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.overlay_position, OverlayPosition::Center);
        assert!(config.live_transcription);
        assert!(config.sound_effects);
        assert!(config.show_accessibility_helper_on_launch);
        assert!(!config.accessibility_helper_dismissed);
    }

    #[test]
    fn hotkey_info_lookup() {
        let config = Config::default();
        let config = Config {
            hotkey: "right_option".to_string(),
            ..config
        };
        let info = config.hotkey_info();
        assert_eq!(info.id, "right_option");
        assert_eq!(info.modifier_flag, 0x00080000);
    }

    #[test]
    fn hotkey_info_fallback() {
        let config = Config {
            hotkey: "nonexistent".to_string(),
            ..Config::default()
        };
        let info = config.hotkey_info();
        assert_eq!(info.id, "left_control");
    }

    #[test]
    fn position_label() {
        let config = Config {
            overlay_position: OverlayPosition::Top,
            ..Config::default()
        };
        assert_eq!(config.position_label(), "Top");
    }

    #[test]
    fn config_from_invalid_json() {
        let result: Result<Config, _> = serde_json::from_str("not json");
        assert!(result.is_err());
    }

    #[test]
    fn normalize_invalid_config_values() {
        let config = Config {
            model: "unknown".to_string(),
            hotkey: "unknown".to_string(),
            ..Config::default()
        }
        .normalized();

        assert_eq!(config.model, "base");
        assert_eq!(config.hotkey, "left_control");
    }
}
