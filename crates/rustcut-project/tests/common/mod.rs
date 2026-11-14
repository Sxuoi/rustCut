//! Shared helpers for the integration tests.
#![allow(dead_code)]

use rustcut_model::{
    AddClipRequest, Clip, ClipOutcome, ExportRecord, ExportStatus, Fingerprint, LastExport, Preset,
    Project, SourceInfo, VideoInfo, now_utc,
};
use rustcut_project::{ProjectStore, StoreOptions};

pub fn source(duration_sec: f64) -> SourceInfo {
    SourceInfo {
        path: "D:/Recordings/test stream.mp4".into(),
        relative_path: "test stream.mp4".into(),
        fingerprint: Fingerprint {
            size_bytes: 987_654_321,
            xxh3_head_tail: "0123456789abcdef".into(),
        },
        duration_sec,
        start_time_sec: 0.0,
        video: VideoInfo {
            codec: "h264".into(),
            coded_width: 1920,
            coded_height: 1080,
            display_width: 1920,
            display_height: 1080,
            rotation: 0,
            sar: "1:1".into(),
            frame_rate: "60/1".into(),
            vfr: false,
            hdr: false,
            stream_index: Some(0),
            pix_fmt: None,
            profile: None,
            level: None,
            color_transfer: None,
            bit_depth: None,
        },
        audio_tracks: Vec::new(),
        format_name: None,
        bit_rate: None,
    }
}

pub fn project(duration_sec: f64) -> Project {
    Project::new(source(duration_sec))
}

pub fn store(duration_sec: f64) -> ProjectStore {
    ProjectStore::new(project(duration_sec), None)
}

pub fn opts() -> StoreOptions {
    StoreOptions::default()
}

pub fn req(start_sec: f64, end_sec: f64) -> AddClipRequest {
    AddClipRequest {
        start_sec,
        end_sec,
        name: None,
        tag_id: None,
    }
}

pub fn add(store: &mut ProjectStore, start_sec: f64, end_sec: f64) -> Clip {
    store
        .add_clip(&req(start_sec, end_sec), &opts())
        .expect("valid clip")
        .clip
}

pub fn clip(index: u32, start_sec: f64, end_sec: f64) -> Clip {
    Clip {
        id: uuid::Uuid::now_v7(),
        index,
        name: format!("Clip {index}"),
        start_sec,
        end_sec,
        tag_id: None,
        crop_x_offset: 0.0,
        selected_for_export: true,
        export_status: ExportStatus::Pending,
        last_export: None,
    }
}

/// A clip exported with `preset` whose stored hash matches its content.
pub fn completed(mut clip: Clip, preset: Preset) -> Clip {
    clip.export_status = ExportStatus::Completed;
    clip.last_export = Some(LastExport {
        output_path: Some(format!("D:/out/{:02}.mp4", clip.index)),
        exported_at: now_utc(),
        settings_hash: "c41f09aa".into(),
        error: None,
        preset: Some(preset),
        content_hash: Some(clip.content_hash(preset)),
        exit_code: None,
        log_tail: Vec::new(),
        log_path: None,
    });
    clip
}

pub fn record(clip: &Clip, outcome: ClipOutcome) -> ExportRecord {
    ExportRecord {
        clip_id: clip.id,
        outcome,
        exported_at: now_utc(),
        preset: Preset::Lossless,
        settings_hash: "abcd1234".into(),
        content_hash: clip.content_hash(Preset::Lossless),
    }
}

pub fn done(path: &str) -> ClipOutcome {
    ClipOutcome::Done {
        output_path: path.into(),
        size_bytes: 1000,
    }
}
