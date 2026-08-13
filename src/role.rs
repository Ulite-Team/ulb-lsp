//! File-role classification for `.ulb` sources (GRAMMAR.md §10).
//!
//! The DSL has one grammar and four file roles — `settings.ulb`,
//! `build.ulb`, `conventions.ulb`, and `libs.ulb`. The parser accepts any
//! file; the roles decide which diagnostics apply to a document (only a
//! `build.ulb` is checked for `apply` targets, for example).

use lsp_types::Url;

/// The four `.ulb` file roles, plus "unknown" for everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// `settings.ulb` — project name, module list, repositories.
    Settings,
    /// `build.ulb` — one per module: `apply`, `deps`, `android`, `task`.
    Build,
    /// `conventions.ulb` — `convention NAME { ... }` and `fn` definitions.
    Conventions,
    /// `libs.ulb` — the version catalog: aliases, `versions {}`, `bundle
    /// {}`, `plugins {}`.
    Libs,
    /// Any other filename. The document is still parsed, but no role-
    /// specific diagnostics apply to it.
    Unknown,
}

impl Role {
    /// Classifies a file by its last path segment.
    ///
    /// # Examples
    ///
    /// ```
    /// use ulb_lsp::role::Role;
    ///
    /// assert_eq!(Role::from_filename("build.ulb"), Role::Build);
    /// assert_eq!(Role::from_filename("conventions.ulb"), Role::Conventions);
    /// assert_eq!(Role::from_filename("README.md"), Role::Unknown);
    /// ```
    #[must_use]
    pub fn from_filename(name: &str) -> Self {
        match name {
            "settings.ulb" => Self::Settings,
            "build.ulb" => Self::Build,
            "conventions.ulb" => Self::Conventions,
            "libs.ulb" => Self::Libs,
            _ => Self::Unknown,
        }
    }
}

/// Classifies the document at `uri` by its filename.
///
/// # Examples
///
/// ```
/// use lsp_types::Url;
/// use ulb_lsp::role::{Role, role_of};
///
/// let url = Url::parse("file:///project/build.ulb").expect("valid file URL");
/// assert_eq!(role_of(&url), Role::Build);
/// ```
#[must_use]
pub fn role_of(uri: &Url) -> Role {
    uri.path_segments()
        .and_then(|mut segments| segments.next_back())
        .map_or(Role::Unknown, Role::from_filename)
}
