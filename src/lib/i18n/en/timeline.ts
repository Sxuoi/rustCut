// English messages for the "timeline" area. Keys are namespaced ("timeline.*" or a
// component prefix). OWNER: see src/lib/i18n/index.ts.
const messages: Record<string, string> = {
  "timeline.loop.asExported": "Loop as exported",
  "timeline.loop.asExportedHint": "Lossless loop preview starts at the effective keyframe when Snap In to keyframe is off.",
  "timeline.readOnly.unwritable": "The project is read-only because its location is not writable.",
  "timeline.overview.label": "Overview timeline",
  "timeline.overview.summary": "{count} clips on a {duration} source. Click to seek or select a clip. Drag the selected clip's edges to trim, hold Shift to snap, Escape cancels.",
  "timeline.zoom.label": "Zoom timeline",
  "timeline.zoom.summary":
    "Detail window of {window} around the playhead. Scroll to zoom, drag to scrub, drag a clip edge to trim, hold Shift to snap, plus and minus keys zoom.",
  "timeline.duplicate": "Duplicate range: another clip has the same In and Out",
  "timeline.exported": "Exported",
  "timeline.leadIn": "+{lead} s",
  "timeline.trim.in": "In",
  "timeline.trim.out": "Out",
  "timeline.snap.playhead": "snapped to playhead",
  "timeline.snap.keyframe": "snapped to keyframe",
  "timeline.snap.edge": "snapped to clip edge",

  "crop.window.label": "9:16 crop window",
  "crop.window.valuetext": "Crop position {offset}",
  "crop.position": "Crop position",
  "crop.reset": "Reset",
  "crop.only916": "9:16 only",
  "crop.hint.noClip": "Select a clip to position the 9:16 window.",
  "crop.hint.ignored": "Crop position ignored: this source is already 9:16 or taller.",
};

export default messages;
