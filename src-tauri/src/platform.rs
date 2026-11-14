//! Operating-system integrations the core needs, behind a trait so the command
//! logic runs without a WebView (tests use [`crate::testing::NullPlatform`]).

use std::path::Path;

/// Asset-protocol scope, notifications and file-manager integration.
pub trait Platform: Send + Sync {
    /// Allow the WebView to read `path` through the asset protocol (PRD 7.9).
    fn allow_asset(&self, path: &Path);
    /// Stop allowing `path`. Tauri's scope has no un-forbid: this is permanent for the process.
    fn forbid_asset(&self, path: &Path);
    /// OS notification (EXP-14).
    fn notify(&self, title: &str, body: &str);
    /// Show `path` selected in the file manager.
    fn reveal(&self, path: &Path) -> Result<(), String>;
    /// Open the folder `path` in the file manager.
    fn open_dir(&self, path: &Path) -> Result<(), String>;
}
