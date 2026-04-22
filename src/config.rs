use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
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

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppAppearance {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryBackendPreference {
    #[default]
    Bundled,
    Ollama,
}

/// Where multimodal “screen help” (Option hotkey) runs: local Gemma or a cloud API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionProvider {
    #[default]
    Local,
    Openai,
    Gemini,
}

pub struct VisionProviderInfo {
    pub id: VisionProvider,
    pub label: &'static str,
}

pub const VISION_PROVIDERS: &[VisionProviderInfo] = &[
    VisionProviderInfo {
        id: VisionProvider::Local,
        label: "Local (Gemma on-device)",
    },
    VisionProviderInfo {
        id: VisionProvider::Openai,
        label: "OpenAI (cloud)",
    },
    VisionProviderInfo {
        id: VisionProvider::Gemini,
        label: "Google Gemini (cloud)",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientFinalBackendPreference {
    #[default]
    Native,
    NativeDiarization,
}

impl AmbientFinalBackendPreference {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::NativeDiarization => "native_diarization",
        }
    }
}

impl Serialize for AmbientFinalBackendPreference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AmbientFinalBackendPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "native" => Ok(Self::Native),
            "native_diarization" | "whisperx_hybrid" => Ok(Self::NativeDiarization),
            other => Err(de::Error::unknown_variant(
                other,
                &["native", "native_diarization", "whisperx_hybrid"],
            )),
        }
    }
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

pub struct AppearanceInfo {
    pub id: AppAppearance,
    pub label: &'static str,
}

pub const APPEARANCES: &[AppearanceInfo] = &[
    AppearanceInfo {
        id: AppAppearance::Dark,
        label: "Dark",
    },
    AppearanceInfo {
        id: AppAppearance::Light,
        label: "Light",
    },
];

pub struct AmbientFinalBackendInfo {
    pub id: AmbientFinalBackendPreference,
    pub label: &'static str,
}

pub const AMBIENT_FINAL_BACKENDS: &[AmbientFinalBackendInfo] = &[
    AmbientFinalBackendInfo {
        id: AmbientFinalBackendPreference::Native,
        label: "Native",
    },
    AmbientFinalBackendInfo {
        id: AmbientFinalBackendPreference::NativeDiarization,
        label: "Native Diarization",
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub hotkey: String,
    #[serde(default)]
    pub overlay_position: OverlayPosition,
    #[serde(default)]
    pub appearance: AppAppearance,
    #[serde(default = "default_live_transcription")]
    pub live_transcription: bool,
    #[serde(default = "default_sound_effects")]
    pub sound_effects: bool,
    #[serde(default = "default_ambient_microphone")]
    pub ambient_microphone: bool,
    #[serde(default = "default_ambient_system_audio")]
    pub ambient_system_audio: bool,
    #[serde(default)]
    pub ambient_final_backend: AmbientFinalBackendPreference,
    #[serde(default)]
    pub summary_backend: SummaryBackendPreference,
    #[serde(default = "default_summary_ollama_model")]
    pub summary_ollama_model: String,
    #[serde(default = "default_show_accessibility_helper_on_launch")]
    pub show_accessibility_helper_on_launch: bool,
    #[serde(default)]
    pub accessibility_helper_dismissed: bool,
    #[serde(default = "default_vision_hotkey")]
    pub vision_hotkey: String,
    #[serde(default)]
    pub vision_provider: VisionProvider,
    #[serde(default = "default_vision_openai_model")]
    pub vision_openai_model: String,
    #[serde(default = "default_vision_gemini_model")]
    pub vision_gemini_model: String,
    /// When true, Screen help runs only the main answer vision call. Skips the second
    /// localization pass (extra cloud round-trip or local helper run) that places the arrow;
    /// highlights fall back to heuristics from the spoken answer text.
    #[serde(default)]
    pub vision_fast_screen_help: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "base".to_string(),
            hotkey: "left_control".to_string(),
            overlay_position: OverlayPosition::default(),
            appearance: AppAppearance::default(),
            live_transcription: default_live_transcription(),
            sound_effects: default_sound_effects(),
            ambient_microphone: default_ambient_microphone(),
            ambient_system_audio: default_ambient_system_audio(),
            ambient_final_backend: AmbientFinalBackendPreference::default(),
            summary_backend: SummaryBackendPreference::default(),
            summary_ollama_model: default_summary_ollama_model(),
            show_accessibility_helper_on_launch: default_show_accessibility_helper_on_launch(),
            accessibility_helper_dismissed: false,
            vision_hotkey: default_vision_hotkey(),
            vision_provider: VisionProvider::default(),
            vision_openai_model: default_vision_openai_model(),
            vision_gemini_model: default_vision_gemini_model(),
            vision_fast_screen_help: false,
        }
    }
}

fn default_live_transcription() -> bool {
    true
}

fn default_sound_effects() -> bool {
    true
}

fn default_ambient_microphone() -> bool {
    true
}

fn default_ambient_system_audio() -> bool {
    true
}

fn default_summary_ollama_model() -> String {
    "gemma4:latest".to_string()
}

fn default_show_accessibility_helper_on_launch() -> bool {
    true
}

fn default_vision_hotkey() -> String {
    "left_option".to_string()
}

fn default_vision_openai_model() -> String {
    "gpt-4o-mini".to_string()
}

fn default_vision_gemini_model() -> String {
    "gemini-3.1-pro-preview".to_string()
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

    /// For `OPENAI_API_KEY` file fallback (GUI apps often lack shell env).
    pub fn resolve_openai_api_key() -> Result<String, String> {
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            let k = k.trim().to_string();
            if !k.is_empty() {
                return Ok(k);
            }
        }
        let path = Self::config_dir().join("openai_api_key");
        if path.is_file() {
            let s = fs::read_to_string(&path).map_err(|e| {
                format!("Could not read OpenAI key file {}: {e}", path.display())
            })?;
            let k = s.lines().next().unwrap_or("").trim().to_string();
            if !k.is_empty() {
                return Ok(k);
            }
        }
        Err(
            "OpenAI API key not configured. Either set the OPENAI_API_KEY environment variable, \
             or put your secret key on a single line in:\n\
             ~/Library/Application Support/Screamer/openai_api_key\n\n\
             When Screen help uses OpenAI, screenshots and prompts are sent to OpenAI. \
             Rotate any key that was exposed elsewhere."
                .to_string(),
        )
    }

    /// For `GEMINI_API_KEY` file fallback (GUI apps often lack shell env).
    pub fn resolve_gemini_api_key() -> Result<String, String> {
        if let Ok(k) = std::env::var("GEMINI_API_KEY") {
            let k = k.trim().to_string();
            if !k.is_empty() {
                return Ok(k);
            }
        }
        let path = Self::config_dir().join("gemini_api_key");
        if path.is_file() {
            let s = fs::read_to_string(&path).map_err(|e| {
                format!("Could not read Gemini key file {}: {e}", path.display())
            })?;
            let k = s.lines().next().unwrap_or("").trim().to_string();
            if !k.is_empty() {
                return Ok(k);
            }
        }
        Err(
            "Gemini API key not configured. Either set the GEMINI_API_KEY environment variable, \
             or put your API key on a single line in:\n\
             ~/Library/Application Support/Screamer/gemini_api_key\n\n\
             When Screen help uses Gemini, screenshots and prompts are sent to Google. \
             See https://ai.google.dev/gemini-api/docs — rotate any key that was exposed elsewhere."
                .to_string(),
        )
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

    pub fn vision_hotkey_info(&self) -> &'static HotkeyInfo {
        HOTKEYS
            .iter()
            .find(|h| h.id == self.vision_hotkey)
            .unwrap_or(&HOTKEYS[2]) // default to left_option
    }

    pub fn vision_hotkey_label(&self) -> &'static str {
        self.vision_hotkey_info().label
    }

    pub fn position_label(&self) -> &'static str {
        POSITIONS
            .iter()
            .find(|p| p.id == self.overlay_position)
            .map(|p| p.label)
            .unwrap_or("Center")
    }

    pub fn appearance_label(&self) -> &'static str {
        APPEARANCES
            .iter()
            .find(|appearance| appearance.id == self.appearance)
            .map(|appearance| appearance.label)
            .unwrap_or("Dark")
    }

    pub fn summary_backend_label(&self) -> &'static str {
        match self.summary_backend {
            SummaryBackendPreference::Bundled => "Bundled Gemma 3 1B",
            SummaryBackendPreference::Ollama => "Local Ollama",
        }
    }

    pub fn ambient_final_backend_label(&self) -> &'static str {
        AMBIENT_FINAL_BACKENDS
            .iter()
            .find(|backend| backend.id == self.ambient_final_backend)
            .map(|backend| backend.label)
            .unwrap_or("Native")
    }

    fn normalized(mut self) -> Self {
        let default = Self::default();

        if !MODELS.iter().any(|model| model.id == self.model) {
            self.model = default.model;
        }

        if !HOTKEYS.iter().any(|hotkey| hotkey.id == self.hotkey) {
            self.hotkey = default.hotkey;
        }

        if !APPEARANCES
            .iter()
            .any(|appearance| appearance.id == self.appearance)
        {
            self.appearance = default.appearance;
        }

        if self.summary_ollama_model.trim().is_empty() {
            self.summary_ollama_model = default.summary_ollama_model;
        }

        if !HOTKEYS.iter().any(|hotkey| hotkey.id == self.vision_hotkey) {
            self.vision_hotkey = default.vision_hotkey;
        }

        if !VISION_PROVIDERS
            .iter()
            .any(|entry| entry.id == self.vision_provider)
        {
            self.vision_provider = default.vision_provider;
        }

        if self.vision_openai_model.trim().is_empty() {
            self.vision_openai_model = default_vision_openai_model();
        }

        if self.vision_gemini_model.trim().is_empty() {
            self.vision_gemini_model = default_vision_gemini_model();
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
        assert_eq!(config.appearance, AppAppearance::Dark);
        assert!(config.live_transcription);
        assert!(config.sound_effects);
        assert!(config.ambient_microphone);
        assert!(config.ambient_system_audio);
        assert_eq!(
            config.ambient_final_backend,
            AmbientFinalBackendPreference::Native
        );
        assert_eq!(config.summary_backend, SummaryBackendPreference::Bundled);
        assert_eq!(config.summary_ollama_model, "gemma4:latest");
        assert!(config.show_accessibility_helper_on_launch);
        assert!(!config.accessibility_helper_dismissed);
        assert_eq!(config.vision_hotkey, "left_option");
        assert_eq!(config.vision_provider, VisionProvider::Local);
        assert_eq!(config.vision_openai_model, "gpt-4o-mini");
        assert_eq!(config.vision_gemini_model, "gemini-3.1-pro-preview");
        assert!(!config.vision_fast_screen_help);
    }

    #[test]
    fn config_roundtrip() {
        let config = Config {
            model: "tiny".to_string(),
            hotkey: "fn".to_string(),
            overlay_position: OverlayPosition::Bottom,
            appearance: AppAppearance::Light,
            live_transcription: false,
            sound_effects: false,
            ambient_microphone: false,
            ambient_system_audio: false,
            ambient_final_backend: AmbientFinalBackendPreference::NativeDiarization,
            summary_backend: SummaryBackendPreference::Ollama,
            summary_ollama_model: "gemma4:e2b".to_string(),
            show_accessibility_helper_on_launch: false,
            accessibility_helper_dismissed: true,
            vision_hotkey: "right_option".to_string(),
            vision_provider: VisionProvider::Openai,
            vision_openai_model: "gpt-4o".to_string(),
            vision_gemini_model: "gemini-2.5-flash".to_string(),
            vision_fast_screen_help: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "tiny");
        assert_eq!(parsed.hotkey, "fn");
        assert_eq!(parsed.overlay_position, OverlayPosition::Bottom);
        assert_eq!(parsed.appearance, AppAppearance::Light);
        assert!(!parsed.live_transcription);
        assert!(!parsed.sound_effects);
        assert!(!parsed.ambient_microphone);
        assert!(!parsed.ambient_system_audio);
        assert_eq!(
            parsed.ambient_final_backend,
            AmbientFinalBackendPreference::NativeDiarization
        );
        assert_eq!(parsed.summary_backend, SummaryBackendPreference::Ollama);
        assert_eq!(parsed.summary_ollama_model, "gemma4:e2b");
        assert!(!parsed.show_accessibility_helper_on_launch);
        assert!(parsed.accessibility_helper_dismissed);
        assert_eq!(parsed.vision_hotkey, "right_option");
        assert_eq!(parsed.vision_provider, VisionProvider::Openai);
        assert_eq!(parsed.vision_openai_model, "gpt-4o");
        assert_eq!(parsed.vision_gemini_model, "gemini-2.5-flash");
        assert!(parsed.vision_fast_screen_help);
    }

    #[test]
    fn config_backward_compat() {
        // Old config without newer fields should deserialize with defaults.
        let json = r#"{"model":"base","hotkey":"left_control"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.overlay_position, OverlayPosition::Center);
        assert_eq!(config.appearance, AppAppearance::Dark);
        assert!(config.live_transcription);
        assert!(config.sound_effects);
        assert!(config.ambient_microphone);
        assert!(config.ambient_system_audio);
        assert_eq!(
            config.ambient_final_backend,
            AmbientFinalBackendPreference::Native
        );
        assert_eq!(config.summary_backend, SummaryBackendPreference::Bundled);
        assert_eq!(config.summary_ollama_model, "gemma4:latest");
        assert!(config.show_accessibility_helper_on_launch);
        assert!(!config.accessibility_helper_dismissed);
        assert_eq!(config.vision_hotkey, "left_option");
        assert_eq!(config.vision_provider, VisionProvider::Local);
        assert_eq!(config.vision_openai_model, "gpt-4o-mini");
        assert_eq!(config.vision_gemini_model, "gemini-3.1-pro-preview");
        assert!(!config.vision_fast_screen_help);
    }

    #[test]
    fn config_parses_gemini_vision_provider() {
        let json = r#"{"model":"base","hotkey":"left_control","vision_provider":"gemini"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.vision_provider, VisionProvider::Gemini);
    }

    #[test]
    fn config_accepts_legacy_whisperx_backend_alias() {
        let config: Config = serde_json::from_str(
            r#"{"model":"base","hotkey":"left_control","ambient_final_backend":"whisperx_hybrid"}"#,
        )
        .unwrap();
        assert_eq!(
            config.ambient_final_backend,
            AmbientFinalBackendPreference::NativeDiarization
        );
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
    fn appearance_label() {
        let config = Config {
            appearance: AppAppearance::Light,
            ..Config::default()
        };
        assert_eq!(config.appearance_label(), "Light");
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
