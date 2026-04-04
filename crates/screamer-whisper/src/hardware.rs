use libc::{c_void, size_t};
use std::ffi::CString;
use whisper_rs::SystemInfo;

const APPLE_SILICON_THREADS: i32 = 4;
const INTEL_AVX2_THREADS: i32 = 6;
const INTEL_LEGACY_THREADS: i32 = 8;
const DEFAULT_CPU_THREADS: i32 = 4;

const APPLE_SILICON_MIN_AUDIO_CTX: i32 = 256;
const INTEL_MIN_AUDIO_CTX: i32 = 384;
const DEFAULT_MIN_AUDIO_CTX: i32 = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComputeBackendPreference {
    CpuOnly,
    PreferGpu,
    GpuOnly,
}

#[derive(Clone, Copy, Debug)]
pub struct RuntimeTuning {
    pub compute_backend: ComputeBackendPreference,
    pub flash_attn: bool,
    pub gpu_device: i32,
    pub n_threads: i32,
    pub adaptive_audio_ctx_min: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
    X86_64,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleChipTier {
    Base,
    Pro,
    Max,
    Ultra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppleChip {
    pub generation: Option<u8>,
    pub tier: AppleChipTier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineFamily {
    AppleSilicon(AppleChip),
    Intel,
    OtherArm,
    Other,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuFeatures {
    pub avx: bool,
    pub avx2: bool,
    pub fma: bool,
    pub f16c: bool,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct MachineProfile {
    pub brand: String,
    pub architecture: Architecture,
    pub family: MachineFamily,
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub performance_cores: Option<usize>,
    pub efficiency_cores: Option<usize>,
    pub translated: bool,
    pub cpu_features: CpuFeatures,
}

impl MachineProfile {
    pub fn detect() -> Self {
        let brand = sysctl_string("machdep.cpu.brand_string")
            .or_else(|| sysctl_string("hw.model"))
            .unwrap_or_else(|| "Unknown Mac".to_string());

        let architecture = detect_architecture();
        let family = classify_family(&brand, architecture);
        let physical_cores = sysctl_usize("hw.physicalcpu")
            .or_else(|| sysctl_usize("hw.ncpu"))
            .unwrap_or(1);
        let logical_cores = sysctl_usize("hw.logicalcpu_max")
            .or_else(|| sysctl_usize("hw.logicalcpu"))
            .or_else(|| sysctl_usize("hw.ncpu"))
            .unwrap_or(physical_cores.max(1));

        let performance_cores = sysctl_usize("hw.perflevel0.physicalcpu");
        let efficiency_cores = sysctl_usize("hw.perflevel1.physicalcpu");
        let translated = sysctl_i32("sysctl.proc_translated").unwrap_or(0) == 1;

        let system = SystemInfo::default();
        let cpu_features = CpuFeatures {
            avx: system.avx,
            avx2: system.avx2,
            fma: system.fma,
            f16c: system.f16c,
        };

        Self {
            brand,
            architecture,
            family,
            physical_cores,
            logical_cores,
            performance_cores,
            efficiency_cores,
            translated,
            cpu_features,
        }
    }

    pub fn recommended_tuning(&self) -> RuntimeTuning {
        match self.family {
            MachineFamily::AppleSilicon(_) => RuntimeTuning {
                compute_backend: if self.translated {
                    ComputeBackendPreference::CpuOnly
                } else {
                    ComputeBackendPreference::GpuOnly
                },
                flash_attn: !self.translated,
                gpu_device: 0,
                n_threads: self
                    .performance_cores
                    .unwrap_or(self.physical_cores)
                    .clamp(1, APPLE_SILICON_THREADS as usize) as i32,
                adaptive_audio_ctx_min: APPLE_SILICON_MIN_AUDIO_CTX,
            },
            MachineFamily::Intel => {
                let cpu_threads = if self.cpu_features.avx2 {
                    INTEL_AVX2_THREADS
                } else {
                    INTEL_LEGACY_THREADS
                };

                RuntimeTuning {
                    compute_backend: ComputeBackendPreference::CpuOnly,
                    flash_attn: false,
                    gpu_device: 0,
                    n_threads: self.physical_cores.clamp(1, cpu_threads as usize) as i32,
                    adaptive_audio_ctx_min: INTEL_MIN_AUDIO_CTX,
                }
            }
            MachineFamily::OtherArm => RuntimeTuning {
                compute_backend: ComputeBackendPreference::PreferGpu,
                flash_attn: true,
                gpu_device: 0,
                n_threads: self.physical_cores.clamp(1, APPLE_SILICON_THREADS as usize) as i32,
                adaptive_audio_ctx_min: APPLE_SILICON_MIN_AUDIO_CTX,
            },
            MachineFamily::Other => RuntimeTuning {
                compute_backend: ComputeBackendPreference::CpuOnly,
                flash_attn: false,
                gpu_device: 0,
                n_threads: self.physical_cores.clamp(1, DEFAULT_CPU_THREADS as usize) as i32,
                adaptive_audio_ctx_min: DEFAULT_MIN_AUDIO_CTX,
            },
        }
    }

    pub fn summary(&self) -> String {
        let arch = match self.architecture {
            Architecture::Arm64 => "arm64",
            Architecture::X86_64 => "x86_64",
            Architecture::Unknown => "unknown",
        };

        let mut parts = vec![format!("{} ({})", self.brand, arch)];

        if let Some(perf) = self.performance_cores {
            if let Some(eff) = self.efficiency_cores {
                parts.push(format!("{}P+{}E", perf, eff));
            }
        } else {
            parts.push(format!("{} cores", self.physical_cores));
        }

        if self.translated {
            parts.push("Rosetta".to_string());
        }

        if matches!(self.family, MachineFamily::Intel) {
            let mut caps = Vec::new();
            if self.cpu_features.avx2 {
                caps.push("AVX2");
            }
            if self.cpu_features.fma {
                caps.push("FMA");
            }
            if self.cpu_features.f16c {
                caps.push("F16C");
            }
            if !caps.is_empty() {
                parts.push(caps.join("/"));
            }
        }

        parts.join(" | ")
    }
}

fn classify_family(brand: &str, architecture: Architecture) -> MachineFamily {
    if let Some(chip) = parse_apple_chip(brand) {
        return MachineFamily::AppleSilicon(chip);
    }

    if brand.contains("Intel") {
        return MachineFamily::Intel;
    }

    match architecture {
        Architecture::Arm64 => MachineFamily::OtherArm,
        Architecture::X86_64 => MachineFamily::Intel,
        Architecture::Unknown => MachineFamily::Other,
    }
}

fn parse_apple_chip(brand: &str) -> Option<AppleChip> {
    let rest = brand.strip_prefix("Apple M")?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    let generation = digits.parse::<u8>().ok();
    let tier = if rest.contains("Ultra") {
        AppleChipTier::Ultra
    } else if rest.contains("Max") {
        AppleChipTier::Max
    } else if rest.contains("Pro") {
        AppleChipTier::Pro
    } else {
        AppleChipTier::Base
    };

    Some(AppleChip { generation, tier })
}

fn detect_architecture() -> Architecture {
    if sysctl_i32("hw.optional.arm64").unwrap_or(0) == 1 {
        Architecture::Arm64
    } else if cfg!(target_arch = "x86_64") {
        Architecture::X86_64
    } else {
        Architecture::Unknown
    }
}

fn sysctl_string(name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    let mut size = 0usize;

    unsafe {
        if libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
            || size == 0
        {
            return None;
        }
    }

    let mut buffer = vec![0u8; size];
    unsafe {
        if libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
    }

    if let Some(0) = buffer.last().copied() {
        buffer.pop();
    }

    String::from_utf8(buffer).ok()
}

fn sysctl_i32(name: &str) -> Option<i32> {
    sysctl_scalar(name)
}

fn sysctl_usize(name: &str) -> Option<usize> {
    if let Some(value) = sysctl_scalar::<u64>(name) {
        return usize::try_from(value).ok();
    }
    sysctl_scalar::<u32>(name).map(|value| value as usize)
}

fn sysctl_scalar<T: Copy>(name: &str) -> Option<T> {
    let name = CString::new(name).ok()?;
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = std::mem::size_of::<T>() as size_t;

    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };

    if status == 0 && size == std::mem::size_of::<T>() {
        Some(unsafe { value.assume_init() })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apple_chip_tiers() {
        assert_eq!(
            parse_apple_chip("Apple M2 Max"),
            Some(AppleChip {
                generation: Some(2),
                tier: AppleChipTier::Max,
            })
        );
        assert_eq!(
            parse_apple_chip("Apple M4"),
            Some(AppleChip {
                generation: Some(4),
                tier: AppleChipTier::Base,
            })
        );
        assert_eq!(
            parse_apple_chip("Apple M3 Pro"),
            Some(AppleChip {
                generation: Some(3),
                tier: AppleChipTier::Pro,
            })
        );
    }

    #[test]
    fn classifies_intel() {
        assert!(matches!(
            classify_family(
                "Intel(R) Core(TM) i9-9980HK CPU @ 2.40GHz",
                Architecture::X86_64
            ),
            MachineFamily::Intel
        ));
    }

    #[test]
    fn intel_tuning_stays_on_cpu_backend() {
        let profile = MachineProfile {
            brand: "Intel(R) Core(TM) i5-8279U CPU @ 2.40GHz".to_string(),
            architecture: Architecture::X86_64,
            family: MachineFamily::Intel,
            physical_cores: 4,
            logical_cores: 8,
            performance_cores: None,
            efficiency_cores: None,
            translated: false,
            cpu_features: CpuFeatures {
                avx: true,
                avx2: true,
                fma: true,
                f16c: true,
            },
        };

        let tuning = profile.recommended_tuning();

        assert!(matches!(
            tuning.compute_backend,
            ComputeBackendPreference::CpuOnly
        ));
        assert!(!tuning.flash_attn);
        assert_eq!(tuning.n_threads, 4);
        assert_eq!(tuning.adaptive_audio_ctx_min, INTEL_MIN_AUDIO_CTX);
    }

    #[test]
    fn translated_apple_silicon_avoids_gpu_backend() {
        let profile = MachineProfile {
            brand: "Apple M2 Max".to_string(),
            architecture: Architecture::Arm64,
            family: MachineFamily::AppleSilicon(AppleChip {
                generation: Some(2),
                tier: AppleChipTier::Max,
            }),
            physical_cores: 8,
            logical_cores: 12,
            performance_cores: Some(8),
            efficiency_cores: Some(4),
            translated: true,
            cpu_features: CpuFeatures::default(),
        };

        let tuning = profile.recommended_tuning();

        assert!(matches!(
            tuning.compute_backend,
            ComputeBackendPreference::CpuOnly
        ));
        assert!(!tuning.flash_attn);
        assert_eq!(tuning.n_threads, APPLE_SILICON_THREADS);
        assert_eq!(tuning.adaptive_audio_ctx_min, APPLE_SILICON_MIN_AUDIO_CTX);
    }
}
