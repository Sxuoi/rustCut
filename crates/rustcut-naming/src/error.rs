//! Typed errors and warnings for naming templates (EXP-05).

use thiserror::Error;

/// Every variable a naming template may use (EXP-05).
pub const VARIABLES: [&str; 9] = [
    "index",
    "name",
    "source_name",
    "start",
    "end",
    "duration",
    "date",
    "tag",
    "preset",
];

/// A naming template that cannot be rendered. Messages are shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    /// The template has no characters.
    #[error("The naming template is empty. Use something like {{index}}_{{name}}.")]
    Empty,
    /// `{foo}` where `foo` is not a variable.
    #[error(
        "Unknown variable {{{name}}} in the naming template. Available variables: {{index}}, {{name}}, {{source_name}}, {{start}}, {{end}}, {{duration}}, {{date}}, {{tag}}, {{preset}}."
    )]
    UnknownVariable {
        /// The text between the braces.
        name: String,
    },
    /// A `{` without a matching `}`, or a `}` without a `{`.
    #[error(
        "The naming template has an unbalanced brace at character {position}. Every {{ needs a closing }} and variables cannot contain braces."
    )]
    UnbalancedBraces {
        /// 1-based character position of the offending brace.
        position: usize,
    },
    /// `{{name}}`, `{a{b}}` and similar.
    #[error(
        "The naming template nests braces at character {position}. Variables cannot be placed inside other variables."
    )]
    NestedBraces {
        /// 1-based character position of the inner `{`.
        position: usize,
    },
}

/// A template that works but probably is not what the user meant.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateWarning {
    /// The template ends with a media extension, which is appended anyway.
    #[error(
        "The template ends with \"{extension}\", but the file extension is added automatically; remove it or the files will end in a doubled extension."
    )]
    EndsWithExtension {
        /// The extension including the dot, lower-case (`.mp4`).
        extension: String,
    },
}
