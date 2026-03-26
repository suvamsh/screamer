use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub hotkey: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "base".to_string(),
            hotkey: "left_control".to_string(),
        }
    }
}

/// Available whisper models with display info
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

/// Available hotkey options with display info and modifier flags
pub struct HotkeyInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// Device-independent modifier flag (e.g. 0x00040000 for Control)
    pub modifier_flag: u64,
    /// Device-specific flag to distinguish left/right (0 means any)
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
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let dir = Self::config_dir();
        let _ = fs::create_dir_all(&dir);
        let path = Self::config_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }

    pub fn hotkey_info(&self) -> &'static HotkeyInfo {
        HOTKEYS.iter()
            .find(|h| h.id == self.hotkey)
            .unwrap_or(&HOTKEYS[0]) // default to left_control
    }

    pub fn hotkey_label(&self) -> &'static str {
        self.hotkey_info().label
    }
}
