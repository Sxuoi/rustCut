# src-tauri: the RustCut desktop shell

Tauri v2 glue around the engine crates. Command logic lives in `src/core` (`AppCore`, no
Tauri types, an `EventSink` and a `Platform` trait for the OS) so `cargo test -p rustcut`
drives the whole flow without a WebView (`tests/e2e.rs`).

| Module | Purpose |
| :--- | :--- |
| `core/` | every command of ARCHITECTURE.md section 4: open/relink/close, clip edits, export, media jobs |
| `commands.rs` | thin `#[tauri::command(rename_all = "snake_case")]` wrappers + `AppState` |
| `shell.rs` | tracing (daily files, 14 kept, stderr in debug), panic hook, notifications, reveal, WebView2 accelerators |
| `commands.txt` | the one list of callable commands: read by `build.rs` (app manifest) and mirrored by `capabilities/default.json` (a unit test keeps both and `generate_handler!` in sync) |

## Launching with a path

* **First launch** (`rustcut.exe "D:\clips\game.mp4"` or a `.rustcut` file, e.g. a file
  association): the path is stored once. The UI calls the command
  `get_launch_path() -> string | null` at startup; it returns the path the first time and
  `null` afterwards.
* **Second launch** while the app runs: the single-instance plugin focuses the main window and
  emits the Tauri event **`open-request`** with the path (string) as payload. The UI listens
  with `listen("open-request", e => open(e.payload))` (`core:event:allow-listen` is granted).

Only the first argument that is not an option and ends in `mp4 mov mkv webm flv ts m2ts m4v avi
rustcut` counts; relative paths resolve against the launching process's working directory.

## Security (PRD 7.9)

* `capabilities/default.json` grants only `core:event:allow-listen/unlisten` and the
  `allow-<command>` permission of each app command. No fs/shell/opener/dialog plugin
  permission is exposed to JavaScript; dialogs, reveal and notifications run in Rust.
* The asset protocol scope starts empty. At runtime it allows the playback cache folder and
  the opened source file only; the previous source is forbidden when relinking.
  Tauri's scope cannot un-forbid, so the old source is *not* forbidden on plain close
  (reopening the same video must keep working); the scope is rebuilt empty at each launch.
* WebView2 browser accelerator keys (F5, Ctrl+R, Ctrl+P, F12...) are switched off through
  `ICoreWebView2Settings3` on Windows, so DevTools shortcuts are unavailable in dev builds too.

## Lifecycle

Startup: `cleanup_stale_with` (journaled `.part` files, stray FFmpeg children) runs before
anything else; the FFmpeg probe and hardware-encoder detection warm up in the background.
Window close and app exit cancel exports and background jobs, flush the auto-saver and
release the project lock before the process ends.
