use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2::msg_send;
use objc2::runtime::{AnyClass, Bool};
use objc2_foundation::NSString;
use std::ffi::c_void;

#[derive(Debug, Clone, Copy)]
pub struct PermissionStatus {
    pub microphone_granted: bool,
    pub accessibility_granted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophonePermissionState {
    Authorized,
    NotDetermined,
    Denied,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrophonePermissionOutcome {
    Granted,
    Prompted,
    Denied,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

const AV_AUTHORIZATION_STATUS_NOT_DETERMINED: isize = 0;
const AV_AUTHORIZATION_STATUS_DENIED: isize = 2;
const AV_AUTHORIZATION_STATUS_AUTHORIZED: isize = 3;

pub fn request_startup_permissions(prompt_for_accessibility: bool) -> PermissionStatus {
    PermissionStatus {
        microphone_granted: has_microphone_permission(),
        accessibility_granted: if prompt_for_accessibility {
            request_accessibility_if_needed()
        } else {
            has_accessibility_permission()
        },
    }
}

pub fn has_accessibility_permission() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn has_microphone_permission() -> bool {
    matches!(
        microphone_permission_state(),
        MicrophonePermissionState::Authorized | MicrophonePermissionState::Unknown
    )
}

pub fn prepare_microphone_permission() -> MicrophonePermissionOutcome {
    match microphone_permission_state() {
        MicrophonePermissionState::Authorized | MicrophonePermissionState::Unknown => {
            MicrophonePermissionOutcome::Granted
        }
        MicrophonePermissionState::NotDetermined => {
            request_microphone_access();
            MicrophonePermissionOutcome::Prompted
        }
        MicrophonePermissionState::Denied => MicrophonePermissionOutcome::Denied,
    }
}

pub fn microphone_permission_state() -> MicrophonePermissionState {
    match microphone_authorization_status() {
        Some(AV_AUTHORIZATION_STATUS_AUTHORIZED) => MicrophonePermissionState::Authorized,
        Some(AV_AUTHORIZATION_STATUS_NOT_DETERMINED) => {
            MicrophonePermissionState::NotDetermined
        }
        Some(AV_AUTHORIZATION_STATUS_DENIED) => MicrophonePermissionState::Denied,
        Some(_) => MicrophonePermissionState::Denied,
        None => MicrophonePermissionState::Unknown,
    }
}

fn microphone_authorization_status() -> Option<isize> {
    let Some(capture_device_class) = AnyClass::get(c"AVCaptureDevice") else {
        eprintln!("[screamer] AVCaptureDevice class unavailable");
        return None;
    };

    let media_type = NSString::from_str("soun");
    Some(unsafe { msg_send![capture_device_class, authorizationStatusForMediaType: &*media_type] })
}

fn request_microphone_access() {
    let Some(capture_device_class) = AnyClass::get(c"AVCaptureDevice") else {
        eprintln!("[screamer] AVCaptureDevice class unavailable");
        return;
    };

    let media_type = NSString::from_str("soun");
    let block = block2::RcBlock::new(move |granted: Bool| {
        eprintln!(
            "[screamer] Microphone permission prompt resolved: {}",
            granted.as_bool()
        );
    });

    unsafe {
        let _: () = msg_send![
            capture_device_class,
            requestAccessForMediaType: &*media_type,
            completionHandler: &*block
        ];
    }
}

fn request_accessibility_if_needed() -> bool {
    if has_accessibility_permission() {
        return true;
    }

    let prompt_key = CFString::new("AXTrustedCheckOptionPrompt");
    let options = CFDictionary::from_CFType_pairs(&[(
        prompt_key.as_CFType(),
        CFBoolean::true_value().as_CFType(),
    )]);

    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const c_void) }
}
