//! Hardware-encoder detection (EXP-13): a 1 s test encode per encoder that the FFmpeg build
//! lists, cached on disk per FFmpeg version.

use std::path::Path;
use std::time::Duration;

use rustcut_ffmpeg::{first_error_line, hw_test_args};
use rustcut_model::{EncoderChoice, HwEncoderInfo};
use rustcut_runner::{RunnerError, run_capture};
use serde::{Deserialize, Serialize};

use crate::MediaService;

/// Each test encode gets this long (EXP-13).
pub const HW_TEST_TIMEOUT: Duration = Duration::from_secs(15);

const CACHE_FILE: &str = "hw_encoders.json";
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct HwCache {
    version: u32,
    ffmpeg_version: String,
    encoders: Vec<HwEncoderInfo>,
}

/// Human label of a hardware encoder.
fn label(encoder: EncoderChoice) -> &'static str {
    match encoder {
        EncoderChoice::H264Nvenc => "NVIDIA NVENC",
        EncoderChoice::H264Amf => "AMD AMF",
        EncoderChoice::H264Qsv => "Intel Quick Sync",
        EncoderChoice::H264Videotoolbox => "Apple VideoToolbox",
        EncoderChoice::H264Vaapi => "VAAPI",
        EncoderChoice::Auto => "Automatic",
        EncoderChoice::Libx264 => "Software (libx264)",
    }
}

/// Preference order of [`best_hw_encoder`](MediaService::best_hw_encoder) (EXP-13).
const PREFERENCE: [EncoderChoice; 5] = [
    EncoderChoice::H264Nvenc,
    EncoderChoice::H264Amf,
    EncoderChoice::H264Qsv,
    EncoderChoice::H264Videotoolbox,
    EncoderChoice::H264Vaapi,
];

/// The most preferred available encoder of `list`: nvenc > amf > qsv > videotoolbox > vaapi.
pub fn pick_best(list: &[HwEncoderInfo]) -> Option<EncoderChoice> {
    PREFERENCE.into_iter().find(|choice| {
        list.iter()
            .any(|i| i.available && Some(i.name.as_str()) == choice.ffmpeg_name())
    })
}

fn load_cache(path: &Path, ffmpeg_version: &str) -> Option<Vec<HwEncoderInfo>> {
    let text = std::fs::read_to_string(path).ok()?;
    let cache: HwCache = serde_json::from_str(&text).ok()?;
    (cache.version == CACHE_VERSION && cache.ffmpeg_version == ffmpeg_version)
        .then_some(cache.encoders)
}

fn save_cache(path: &Path, ffmpeg_version: &str, encoders: &[HwEncoderInfo]) {
    let cache = HwCache {
        version: CACHE_VERSION,
        ffmpeg_version: ffmpeg_version.to_string(),
        encoders: encoders.to_vec(),
    };
    let result = serde_json::to_vec_pretty(&cache)
        .map_err(std::io::Error::other)
        .and_then(|bytes| crate::write_atomic(path, &bytes));
    if let Err(err) = result {
        tracing::warn!(path = %path.display(), %err, "could not cache the hardware encoder list");
    }
}

impl MediaService {
    /// Tests every hardware encoder in [`EncoderChoice::HARDWARE`] that the FFmpeg build lists
    /// with a 1 s encode (`rustcut_ffmpeg::hw_test_args`, 15 s timeout, EXP-13). Encoders the
    /// build does not contain are not listed. The result is cached in memory and in
    /// `<data>/hw_encoders.json`, keyed by the FFmpeg version; `force` re-runs the tests.
    ///
    /// When FFmpeg's capabilities cannot be read the list is empty (software encoding only), the
    /// problem is logged and nothing is cached.
    pub async fn detect_hw_encoders(&self, force: bool) -> Vec<HwEncoderInfo> {
        let mut memo = self.inner.hw.lock().await;
        if !force && let Some(list) = memo.as_ref() {
            return list.clone();
        }
        let caps = match rustcut_runner::probe_capabilities(&self.inner.config.sidecars).await {
            Ok(caps) => caps,
            Err(err) => {
                tracing::warn!(%err, "hardware encoder detection could not read the FFmpeg capabilities");
                return Vec::new();
            }
        };
        let cache_path = self.inner.config.data_dir.join(CACHE_FILE);
        if !force && let Some(list) = load_cache(&cache_path, &caps.ffmpeg_version) {
            *memo = Some(list.clone());
            return list;
        }
        let mut found = Vec::new();
        for encoder in EncoderChoice::HARDWARE {
            let Some(name) = encoder.ffmpeg_name() else {
                continue;
            };
            if !caps.has_encoder(name) {
                continue;
            }
            found.push(self.test_encoder(encoder, name).await);
        }
        save_cache(&cache_path, &caps.ffmpeg_version, &found);
        *memo = Some(found.clone());
        found
    }

    async fn test_encoder(&self, encoder: EncoderChoice, name: &str) -> HwEncoderInfo {
        let (available, error) = match hw_test_args(encoder) {
            None => (
                false,
                Some("no test encode is defined for this encoder".to_string()),
            ),
            Some(args) => {
                match run_capture(&self.inner.config.sidecars.ffmpeg, &args, HW_TEST_TIMEOUT).await
                {
                    Ok(out) if out.success() => (true, None),
                    Ok(out) => {
                        let detail = first_error_line(&out.stderr_lossy())
                            .unwrap_or_else(|| "the test encode failed".to_string());
                        (false, Some(detail))
                    }
                    Err(RunnerError::Timeout { .. }) => (
                        false,
                        Some(format!(
                            "the test encode did not finish within {} s",
                            HW_TEST_TIMEOUT.as_secs()
                        )),
                    ),
                    Err(err) => (false, Some(err.to_string())),
                }
            }
        };
        HwEncoderInfo {
            name: name.to_string(),
            label: label(encoder).to_string(),
            available,
            error,
        }
    }

    /// The best available hardware encoder (nvenc > amf > qsv > videotoolbox > vaapi), running
    /// or reusing the detection; `None` when no hardware encoder works (use libx264).
    pub async fn best_hw_encoder(&self) -> Option<EncoderChoice> {
        pick_best(&self.detect_hw_encoders(false).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, available: bool) -> HwEncoderInfo {
        HwEncoderInfo {
            name: name.into(),
            label: String::new(),
            available,
            error: None,
        }
    }

    #[test]
    fn exp13_preference_nvenc_amf_qsv_videotoolbox_vaapi() {
        let all = [
            info("h264_vaapi", true),
            info("h264_videotoolbox", true),
            info("h264_qsv", true),
            info("h264_amf", true),
            info("h264_nvenc", true),
        ];
        assert_eq!(pick_best(&all), Some(EncoderChoice::H264Nvenc));
        assert_eq!(pick_best(&all[..4]), Some(EncoderChoice::H264Amf));
        assert_eq!(pick_best(&all[..3]), Some(EncoderChoice::H264Qsv));
        assert_eq!(pick_best(&all[..2]), Some(EncoderChoice::H264Videotoolbox));
        assert_eq!(pick_best(&all[..1]), Some(EncoderChoice::H264Vaapi));
    }

    #[test]
    fn unavailable_encoders_are_skipped() {
        let list = [info("h264_nvenc", false), info("h264_amf", true)];
        assert_eq!(pick_best(&list), Some(EncoderChoice::H264Amf));
        assert_eq!(pick_best(&[info("h264_nvenc", false)]), None);
        assert_eq!(pick_best(&[]), None);
    }

    #[test]
    fn cache_is_keyed_by_ffmpeg_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CACHE_FILE);
        save_cache(&path, "9.0.2", &[info("h264_amf", true)]);
        assert_eq!(load_cache(&path, "9.0.2").unwrap().len(), 1);
        assert!(load_cache(&path, "9.1").is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_cache(&path, "9.0.2").is_none());
    }
}
