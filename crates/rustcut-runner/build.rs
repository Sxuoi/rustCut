//! Exports the compilation target triple as `RUSTCUT_TARGET_TRIPLE`, so that
//! `Sidecars::locate` can find development sidecars named `ffmpeg-<triple>[.exe]`
//! (Tauri's `externalBin` naming before bundling strips the triple).

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").expect("cargo sets TARGET for every build script");
    println!("cargo::rustc-env=RUSTCUT_TARGET_TRIPLE={target}");
}
