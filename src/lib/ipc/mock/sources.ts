// Fake sources for the mock backend: the generated sample clip and a
// plausible livestream for every other path.

import type { Fingerprint, PlaybackInfo, PlaybackTier, Project, SourceInfo } from "$lib/bindings";
import { APP_VERSION, SCHEMA_VERSION, defaultExportSettings, defaultTags, defaultViewState } from "./model";
import { baseName, dirName, hash32, stripExtension } from "./util";

export const SAMPLE_PATH = "/sample/sample.mp4";
export const SAMPLE_MIME = 'video/mp4; codecs="avc1.64001f, mp4a.40.2"';
export const SAMPLE_DURATION_SEC = 180;
/** Fake video the "Open video" dialog and "browse" return. */
export const FAKE_OUTPUT_DIR = "C:/Users/Demo/Videos/clips";

function fingerprintFor(path: string, size: number): Fingerprint {
  return { size_bytes: size, xxh3_head_tail: `${hash32(path)}${hash32(`${path}#${size}`)}` };
}

/** Probe result of `public/sample/sample.mp4` (scripts/make-sample.mjs). */
export function sampleSource(): SourceInfo {
  return {
    path: SAMPLE_PATH,
    relative_path: "sample.mp4",
    fingerprint: fingerprintFor(SAMPLE_PATH, 4_323_483),
    duration_sec: SAMPLE_DURATION_SEC,
    start_time_sec: 0,
    video: {
      codec: "h264",
      coded_width: 1280,
      coded_height: 720,
      display_width: 1280,
      display_height: 720,
      rotation: 0,
      sar: "1:1",
      frame_rate: "30/1",
      vfr: false,
      hdr: false,
      stream_index: 0,
      pix_fmt: "yuv420p",
      profile: "High",
      level: 31,
      color_transfer: null,
      bit_depth: 8,
    },
    audio_tracks: [
      { stream_index: 1, codec: "aac", channels: 2, title: "Mix", language: null, sample_rate: 48000, is_default: true },
    ],
    format_name: "mov,mp4,m4a,3gp,3g2,mj2",
    bit_rate: 192_155,
  };
}

/** Any other path: a 2:30:00 1080p60 livestream recording with two audio tracks. */
export function fakeSource(path: string): SourceInfo {
  const name = baseName(path);
  return {
    path,
    relative_path: name,
    fingerprint: fingerprintFor(path, 6_750_000_000),
    duration_sec: 9000,
    start_time_sec: 0,
    video: {
      codec: "h264",
      coded_width: 1920,
      coded_height: 1080,
      display_width: 1920,
      display_height: 1080,
      rotation: 0,
      sar: "1:1",
      frame_rate: "60/1",
      vfr: false,
      hdr: false,
      stream_index: 0,
      pix_fmt: "yuv420p",
      profile: "High",
      level: 42,
      color_transfer: "bt709",
      bit_depth: 8,
    },
    audio_tracks: [
      { stream_index: 1, codec: "aac", channels: 2, title: "Game", language: null, sample_rate: 48000, is_default: true },
      { stream_index: 2, codec: "aac", channels: 2, title: "Mic", language: null, sample_rate: 48000, is_default: false },
    ],
    format_name: "matroska,webm",
    bit_rate: 6_000_000,
  };
}

/** The source for a path (the sample gets its real probe values). */
export function sourceFor(path: string): SourceInfo {
  return path === SAMPLE_PATH ? sampleSource() : fakeSource(path);
}

/** PRJ-02: the project file lives next to the video. */
export function projectPathFor(sourcePath: string): string {
  const dir = dirName(sourcePath);
  return `${dir === "/" ? "" : dir}/${stripExtension(baseName(sourcePath))}.rustcut`;
}

/** `<source folder>/<source name>_clips` (§6.2). */
export function defaultOutputDir(source: SourceInfo): string {
  const dir = dirName(source.path);
  const base = `${stripExtension(baseName(source.path))}_clips`;
  // The sample lives on the dev server, so its "folder" is a fake user folder.
  return source.path === SAMPLE_PATH ? `C:/Users/Demo/Videos/${base}` : `${dir}/${base}`;
}

/** A fresh project: default tags and settings (PRJ-02). */
export function newProject(source: SourceInfo, spacesToUnderscores: boolean): Project {
  const now = new Date().toISOString();
  return {
    schema_version: SCHEMA_VERSION,
    app_version: APP_VERSION,
    created_at: now,
    updated_at: now,
    source,
    tags: defaultTags(),
    export_settings: { ...defaultExportSettings(), spaces_to_underscores: spacesToUnderscores },
    next_index: 1,
    clips: [],
    view_state: defaultViewState(),
  };
}

const NEXT_TIER: Record<PlaybackTier, PlaybackTier | null> = { direct: "remux", remux: "proxy", proxy: null };

/** PLY-01: what the player should load for `tier`. */
export function playbackInfo(source: SourceInfo, tier: PlaybackTier): PlaybackInfo {
  return {
    tier,
    media_path: source.path,
    mime_type: SAMPLE_MIME,
    preparing: false,
    progress: null,
    eta_sec: null,
    next_tier: NEXT_TIER[tier],
    unsupported_reason: null,
  };
}
