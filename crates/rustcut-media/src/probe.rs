//! Source probe (PLY-01): FFprobe headers plus the head/tail fingerprint, never the whole file
//! (PRD section 8.1).

use std::path::Path;
use std::time::{Duration, SystemTime};

use rustcut_ffmpeg::{ProbeError, mime_type, parse_probe_json, probe_args, probe_failure_message};
use rustcut_model::{SourceInfo, compute_fingerprint};
use rustcut_runner::{RunnerError, run_capture};

use crate::{MediaError, MediaService, file_name};

/// FFprobe gets this long (a slow or disconnected network drive is reported, not waited for).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// A file modified this recently may still be written by a recorder.
const RECENTLY_MODIFIED: Duration = Duration::from_secs(5);

/// Result of [`MediaService::probe_source`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProbedSource {
    /// Stream facts with the fingerprint filled in; `relative_path` is left empty (the project
    /// knows its own folder).
    pub source: SourceInfo,
    /// RFC 6381 MIME type with the exact codec string, for `canPlayType` (section 7.3).
    pub mime_type: String,
    /// User-facing non-fatal findings (HDR, VFR, ignored streams, a file still being written...).
    pub warnings: Vec<String>,
}

impl MediaService {
    /// Probes `path` (PLY-01): FFprobe with a 15 s timeout, plus the fingerprint (first and last
    /// MiB only), concurrently.
    ///
    /// # Errors
    /// * [`MediaError::Probe`] when FFprobe cannot read the file; the message names the file and
    ///   carries a recovery hint (a truncated MP4 says its index is missing, AC-12).
    /// * [`MediaError::NoVideo`] for audio-only files.
    /// * [`MediaError::Io`] when the file does not exist or cannot be read.
    pub async fn probe_source(&self, path: &Path) -> Result<ProbedSource, MediaError> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| MediaError::io(path, e))?;
        if !meta.is_file() {
            return Err(MediaError::Probe {
                message: format!("{} is not a file", path.display()),
            });
        }
        let fingerprint_path = path.to_path_buf();
        let fingerprint =
            tokio::task::spawn_blocking(move || compute_fingerprint(&fingerprint_path));
        let args = probe_args(path);
        let (output, fingerprint) = tokio::join!(
            run_capture(&self.inner.config.sidecars.ffprobe, &args, PROBE_TIMEOUT),
            fingerprint
        );
        let output = match output {
            Ok(o) => o,
            Err(RunnerError::Timeout { .. }) => {
                return Err(MediaError::Probe {
                    message: format!(
                        "Reading {} took longer than {} s. The file may be on a slow or disconnected drive; \
                         copy it to a local disk and try again.",
                        file_name(path),
                        PROBE_TIMEOUT.as_secs()
                    ),
                });
            }
            Err(e) => return Err(e.into()),
        };
        if !output.success() {
            return Err(MediaError::Probe {
                message: probe_failure_message(&output.stderr_lossy(), path),
            });
        }
        let probe = parse_probe_json(&output.stdout_lossy(), path).map_err(|e| match e {
            ProbeError::NoVideo { .. } => MediaError::NoVideo {
                message: e.to_string(),
            },
            other => MediaError::Probe {
                message: other.to_string(),
            },
        })?;
        let fingerprint = fingerprint
            .map_err(|e| MediaError::io(path, std::io::Error::other(e)))?
            .map_err(|e| MediaError::io(path, e))?;

        let mut source = probe.source;
        source.fingerprint = fingerprint;
        source.relative_path = String::new();
        let mut warnings: Vec<String> = probe.warnings.iter().map(|w| w.message()).collect();
        if recently_modified(&meta) {
            warnings.push(format!(
                "{} was modified moments ago; if it is still being recorded, it is not supported \
                 in this version and the preview and exports may end early. Wait until the \
                 recording has finished.",
                file_name(path)
            ));
        }
        let mime_type = mime_type(&source);
        Ok(ProbedSource {
            source,
            mime_type,
            warnings,
        })
    }
}

fn recently_modified(meta: &std::fs::Metadata) -> bool {
    meta.modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age < RECENTLY_MODIFIED)
}
