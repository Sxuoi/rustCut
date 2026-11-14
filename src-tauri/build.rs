//! Build script: bundles the Tauri context and restricts the IPC surface to the
//! commands listed in `commands.txt` (PRD 7.9: only the app's own commands are
//! callable from the WebView; each one also needs an `allow-*` permission in
//! `capabilities/default.json`).

use tauri_build::{AppManifest, Attributes};

fn main() {
    println!("cargo:rerun-if-changed=commands.txt");
    let text = std::fs::read_to_string("commands.txt").expect("commands.txt is part of the crate");
    let names: Vec<&'static str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| &*Box::leak(l.to_string().into_boxed_str()))
        .collect();
    let names: &'static [&'static str] = Box::leak(names.into_boxed_slice());
    tauri_build::try_build(Attributes::new().app_manifest(AppManifest::new().commands(names)))
        .expect("failed to run the Tauri build script");
}
