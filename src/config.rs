use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    Center,
    Top,
    Bottom,
}

impl Default for OverlayPosition {
    fn default() -> Self {
        Self::Center
    }
}

pub struct PositionInfo {
    pub id: OverlayPosition,
    pub label: &'static str,
}

pub const POSITIONS: &[PositionInfo] = &[
    PositionInfo { id: OverlayPosition::Center, label: "Center" },
    PositionInfo { id: OverlayPosition::Top,    label: "Top" },
    PositionInfo { id: OverlayPosition::Bottom, label: "Bottom" },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub hotkey: String,
    #[serde(default)]
    pub overlay_position: OverlayPosition,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "base".to_string(),
            hotkey: "left_control".to_string(),
            overlay_position: OverlayPosition::default(),
        }
    }
}

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub size: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo { id: "tiny",  label: "Tiny",   size: "75 MB"  },
    ModelInfo { id: "base",  label: "Base",   size: "142 MB" },
    ModelInfo { id: "small", label: "Small",  size: "466 MB" },
];

pub struct HotkeyInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub modifier_flag: u64,
    pub device_flag: u64,
}

pub const HOTKEYS: &[HotkeyInfo] = &[
    HotkeyInfo { id: "left_control",  label: "Left Control ⌃",    modifier_flag: 0x00040000, device_flag: 0x00000001 },
    HotkeyInfo { id: "right_control", label: "Right Control ⌃",   modifier_flag: 0x00040000, device_flag: 0x00002000 },
    HotkeyInfo { id: "left_option",   label: "Left Option ⌥",     modifier_flag: 0x00080000, device_flag: 0x00000020 },
    HotkeyInfo { id: "right_option",  label: "Right Option ⌥",    modifier_flag: 0x00080000, device_flag: 0x00000040 },
    HotkeyInfo { id: "left_command",  label: "Left Command ⌘",    modifier_flag: 0x00100000, device_flag: 0x00000008 },
    HotkeyInfo { id: "right_command", label: "Right Command ⌘",   modifier_flag: 0x00100000, device_flag: 0x00000010 },
    HotkeyInfo { id: "fn",            label: "Fn (Globe) 🌐",     modifier_flag: 0x00800000, device_flag: 0 },
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
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = Self::config_dir();
        let _ = fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let path = Self::config_path();
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, &json).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
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
    }

    #[test]
    fn config_roundtrip() {
        let config = Config {
            model: "tiny".to_string(),
            hotkey: "fn".to_string(),
            overlay_position: OverlayPosition::Bottom,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "tiny");
        assert_eq!(parsed.hotkey, "fn");
        assert_eq!(parsed.overlay_position, OverlayPosition::Bottom);
    }

    #[test]
    fn config_backward_compat() {
        // Old config without overlay_position should deserialize with default
        let json = r#"{"model":"base","hotkey":"left_control"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.overlay_position, OverlayPosition::Center);
    }

    #[test]
    fn hotkey_info_lookup() {
        let config = Config::default();
        let config = Config { hotkey: "right_option".to_string(), ..config };
        let info = config.hotkey_info();
        assert_eq!(info.id, "right_option");
        assert_eq!(info.modifier_flag, 0x00080000);
    }

    #[test]
    fn hotkey_info_fallback() {
        let config = Config { hotkey: "nonexistent".to_string(), ..Config::default() };
        let info = config.hotkey_info();
        assert_eq!(info.id, "left_control");
    }

    #[test]
    fn position_label() {
        let config = Config { overlay_position: OverlayPosition::Top, ..Config::default() };
        assert_eq!(config.position_label(), "Top");
    }

    #[test]
    fn config_from_invalid_json() {
        let result: Result<Config, _> = serde_json::from_str("not json");
        assert!(result.is_err());
    }
}
