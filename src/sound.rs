use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::{MainThreadMarker, NSString};

const RECORDING_START_SOUND: &str = "Pop";
const PROCESSING_DONE_SOUND: &str = "Glass";
const RECORDING_START_VOLUME: f32 = 0.55;
const PROCESSING_DONE_VOLUME: f32 = 0.50;

pub struct SoundPlayer {
    recording_start: Option<Retained<NSSound>>,
    processing_done: Option<Retained<NSSound>>,
}

impl SoundPlayer {
    pub fn new(_mtm: MainThreadMarker) -> Self {
        Self {
            recording_start: load_sound(RECORDING_START_SOUND, RECORDING_START_VOLUME),
            processing_done: load_sound(PROCESSING_DONE_SOUND, PROCESSING_DONE_VOLUME),
        }
    }

    pub fn play_recording_start(&self) {
        play_sound(self.recording_start.as_ref(), RECORDING_START_SOUND);
    }

    pub fn play_processing_done(&self) {
        play_sound(self.processing_done.as_ref(), PROCESSING_DONE_SOUND);
    }
}

fn load_sound(name: &str, volume: f32) -> Option<Retained<NSSound>> {
    let sound = NSSound::soundNamed(&NSString::from_str(name));
    if let Some(sound) = sound.as_ref() {
        sound.setVolume(volume);
    } else {
        eprintln!("[screamer] Failed to load sound cue: {}", name);
    }
    sound
}

fn play_sound(sound: Option<&Retained<NSSound>>, label: &str) {
    let Some(sound) = sound else {
        return;
    };

    sound.stop();
    sound.setCurrentTime(0.0);

    if !sound.play() {
        eprintln!("[screamer] Failed to play sound cue: {}", label);
    }
}
