//! File naming (EXP-05 template, EXP-06 sanitization, EXP-07 collisions) and
//! timestamp import/export (EXP-16/17).
//!
//! * [`sanitize_component`]: EXP-06 steps 1-6, identical on every OS.
//! * [`render_name`] / [`validate_template`] / [`preview_names`]: EXP-05.
//! * [`resolve_outputs`]: EXP-07 collisions plus the Windows path-length edge
//!   case (PRD section 9).
//! * [`timestamps`]: EXP-16 export (chapters, CSV, EDL, FCPXML) and EXP-17
//!   import.
//!
//! The crate is pure: file-system state is injected as a closure so every rule
//! is testable without touching the disk or depending on the host OS.

mod error;
mod resolve;
mod sanitize;
mod template;
pub mod timestamps;

pub use error::{TemplateError, TemplateWarning, VARIABLES};
pub use resolve::{NameRequest, ResolvedName, resolve_outputs};
pub use sanitize::{MAX_COMPONENT_BYTES, sanitize_component};
pub use template::{NameContext, preview_names, render_name, validate_template};
