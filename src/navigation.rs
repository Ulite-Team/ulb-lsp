//! Cross-file navigation: hover and goto-definition for `apply "name"`.
//!
//! An `apply` statement names a `convention` defined in the adjacent
//! `conventions.ulb` (GRAMMAR.md §6.3: everything declared there is
//! globally visible, no imports). These helpers locate the `apply` target
//! under a cursor and resolve it to the definition site, so the editor can
//! preview the convention body and jump to it. They are engine methods
//! ([`DiagnosticEngine::hover`], [`DiagnosticEngine::goto_definition`]) so
//! they share the same open-document store and file loader as diagnostics.

use std::collections::BTreeMap;

use lsp_types::{
    GotoDefinitionResponse, Hover, HoverContents, Location, MarkupContent, MarkupKind, Position,
    Url,
};
use ulb_lang::ast::{ElseBranch, IfKind, Statement, StatementKind};
use ulb_lang::parser::parse;
use ulb_lang::span::Span;

use crate::diagnostics::{DiagnosticEngine, SourceLoader};
use crate::utf16::{position_to_offset, span_to_range};

/// The definition site of one convention, used for hover and
/// goto-definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ConventionLocation {
    /// The `conventions.ulb` document that defines it.
    pub uri: Url,
    /// Span of the `convention NAME` name identifier.
    pub name_span: Span,
    /// Span of the body block (`{ ... }` inclusive).
    pub body_span: Span,
}

impl<L: SourceLoader> DiagnosticEngine<L> {
    /// Hover content for the construct under `position` in the document at
    /// `uri`. An `apply` statement is rendered with its convention's name
    /// and body; a plugin-owned key inside a `build.ulb` block is rendered
    /// with its config-schema type and description (see
    /// [`crate::completion`]). Anything else produces no hover.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use std::path::{Path, PathBuf};
    /// use lsp_types::{HoverContents, MarkupKind, Position, Url};
    /// use ulb_lsp::diagnostics::{DiagnosticEngine, SourceLoader};
    /// use ulb_lsp::document::Document;
    ///
    /// struct MapLoader(HashMap<PathBuf, String>);
    /// impl SourceLoader for MapLoader {
    ///     fn load(&self, path: &Path) -> Option<String> {
    ///         self.0.get(path).cloned()
    ///     }
    /// }
    ///
    /// let mut loader = HashMap::new();
    /// loader.insert(
    ///     PathBuf::from("/proj/conventions.ulb"),
    ///     "convention signed {\n  minifyEnabled true\n}\n".to_owned(),
    /// );
    /// let mut engine = DiagnosticEngine::with_loader(MapLoader(loader));
    /// let build = Url::from_file_path("/proj/build.ulb").expect("file URL");
    /// engine.upsert(
    ///     build.clone(),
    ///     Document::new("apply \"signed\"\n".to_owned(), 1),
    /// );
    ///
    /// let hover = engine.hover(&build, Position::new(0, 7)).expect("hover");
    /// let HoverContents::Markup(markup) = hover.contents else {
    ///     panic!("expected markup hover");
    /// };
    /// assert_eq!(markup.kind, MarkupKind::Markdown);
    /// assert!(markup.value.contains("convention `signed`"));
    /// assert!(markup.value.contains("minifyEnabled"));
    /// ```
    #[must_use]
    pub fn hover(&self, uri: &Url, position: Position) -> Option<Hover> {
        let text = self.text_of(uri)?;
        let Some(target) = apply_target_at(&text, position) else {
            return self.plugin_field_hover(uri, position);
        };
        let name_range = span_to_range(&text, target.name_span);
        let conventions_uri = uri.join("conventions.ulb").ok()?;
        let conventions_text = self.text_of(&conventions_uri);
        let locations = conventions_text
            .as_deref()
            .map(|t| convention_locations(t, &conventions_uri))
            .unwrap_or_default();
        let contents = match locations.get(&target.name) {
            Some(location) => {
                let body = conventions_text
                    .as_deref()
                    .map(|t| {
                        let start = usize::try_from(location.body_span.start)
                            .expect("span offset fits usize");
                        let end = usize::try_from(location.body_span.end)
                            .expect("span offset fits usize");
                        &t[start..end]
                    })
                    .unwrap_or("");
                HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("**convention `{}`**\n```ulb\n{body}\n```", target.name),
                })
            }
            None => HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("convention `{}` is not defined", target.name),
            }),
        };
        Some(Hover {
            contents,
            range: Some(name_range),
        })
    }

    /// The definition location of the convention named by an `apply`
    /// statement under `position` in the document at `uri`: the
    /// `convention NAME` identifier in the adjacent `conventions.ulb`.
    /// Returns `None` for an `apply` to an undefined convention.
    #[must_use]
    pub fn goto_definition(&self, uri: &Url, position: Position) -> Option<GotoDefinitionResponse> {
        let text = self.text_of(uri)?;
        let target = apply_target_at(&text, position)?;
        let conventions_uri = uri.join("conventions.ulb").ok()?;
        let conventions_text = self.text_of(&conventions_uri)?;
        let locations = convention_locations(&conventions_text, &conventions_uri);
        let location = locations.get(&target.name)?;
        Some(GotoDefinitionResponse::Scalar(Location {
            uri: location.uri.clone(),
            range: span_to_range(&conventions_text, location.name_span),
        }))
    }
}

/// An `apply` statement hit under a cursor.
struct ApplyTarget {
    name: String,
    name_span: Span,
}

/// Finds the `apply` statement whose target string contains the byte
/// offset `offset`, walking into blocks and `if`/`else` branches.
fn apply_at_offset(statements: &[Statement], offset: u32) -> Option<ApplyTarget> {
    for stmt in statements {
        if let StatementKind::Apply { name, name_span } = &stmt.kind
            && contains(*name_span, offset)
        {
            return Some(ApplyTarget {
                name: name.clone(),
                name_span: *name_span,
            });
        }
        match &stmt.kind {
            StatementKind::BlockStmt { block, .. } => {
                if let Some(found) = apply_at_offset(&block.statements, offset) {
                    return Some(found);
                }
            }
            StatementKind::TaskDef { block, .. } => {
                if let Some(found) = apply_at_offset(&block.statements, offset) {
                    return Some(found);
                }
            }
            StatementKind::ConventionDef { block, .. } => {
                if let Some(found) = apply_at_offset(&block.statements, offset) {
                    return Some(found);
                }
            }
            StatementKind::FnDef { block, .. } => {
                if let Some(found) = apply_at_offset(&block.statements, offset) {
                    return Some(found);
                }
            }
            StatementKind::If(if_kind) => {
                if let Some(found) = apply_at_offset_if(if_kind, offset) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds an `apply` under `offset` inside an `if`/`else` chain.
fn apply_at_offset_if(if_kind: &IfKind, offset: u32) -> Option<ApplyTarget> {
    if let Some(found) = apply_at_offset(&if_kind.then_branch.statements, offset) {
        return Some(found);
    }
    match &if_kind.else_branch {
        None => None,
        Some(ElseBranch::Block(block)) => apply_at_offset(&block.statements, offset),
        Some(ElseBranch::If(inner)) => apply_at_offset_if(&inner.kind, offset),
    }
}

/// Resolves `position` in `text` to the `apply` target under the cursor,
/// if any.
fn apply_target_at(text: &str, position: Position) -> Option<ApplyTarget> {
    let offset = position_to_offset(text, position)?;
    let parsed = parse(text);
    apply_at_offset(&parsed.file.statements, offset)
}

/// Maps every top-level `convention NAME { ... }` in `text` to its
/// definition site.
fn convention_locations(text: &str, uri: &Url) -> BTreeMap<String, ConventionLocation> {
    let parsed = parse(text);
    let mut out = BTreeMap::new();
    for stmt in &parsed.file.statements {
        if let StatementKind::ConventionDef { name, block } = &stmt.kind {
            out.insert(
                name.text.clone(),
                ConventionLocation {
                    uri: uri.clone(),
                    name_span: name.span,
                    body_span: block.span,
                },
            );
        }
    }
    out
}

/// Whether the half-open `span` contains `offset`.
fn contains(span: Span, offset: u32) -> bool {
    span.start <= offset && offset < span.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn url(path: &str) -> Url {
        Url::from_file_path(path).expect("absolute file path")
    }

    const CONVENTIONS: &str = r#"
convention androidApp {
  android { compileSdk 37 }
}
convention signed {
  minifyEnabled true
}
"#;

    fn engine_with_conventions() -> DiagnosticEngine<MapLoader> {
        let mut loader = HashMap::new();
        loader.insert(
            PathBuf::from("/proj/conventions.ulb"),
            CONVENTIONS.to_owned(),
        );
        DiagnosticEngine::with_loader(MapLoader(loader))
    }

    struct MapLoader(HashMap<PathBuf, String>);

    impl SourceLoader for MapLoader {
        fn load(&self, path: &Path) -> Option<String> {
            self.0.get(path).cloned()
        }
    }

    #[test]
    fn apply_target_is_found_inside_blocks_and_conditionals() {
        let text = "if debugBuild {\n  apply \"signed\"\n}\nandroid {\n  apply \"androidApp\"\n}\n";
        let parsed = parse(text);
        // Inside `"signed"` on line 1 (bytes 24..32).
        let hit = apply_at_offset(&parsed.file.statements, 26);
        assert_eq!(hit.expect("hit in if branch").name, "signed");
        // Inside `"androidApp"` on line 4 (bytes 53..66).
        let hit = apply_at_offset(&parsed.file.statements, 60);
        assert_eq!(hit.expect("hit in block").name, "androidApp");
        // A position outside any apply (the `if` keyword).
        assert!(apply_at_offset(&parsed.file.statements, 0).is_none());
    }

    #[test]
    fn hover_reports_unknown_convention() {
        let mut loader = HashMap::new();
        loader.insert(
            PathBuf::from("/proj/conventions.ulb"),
            CONVENTIONS.to_owned(),
        );
        let mut engine = DiagnosticEngine::with_loader(MapLoader(loader));
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"ghost\"\n".to_owned(), 1),
        );
        let hover = engine.hover(&build, Position::new(0, 8)).expect("hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markup hover");
        };
        assert!(markup.value.contains("`ghost` is not defined"));
    }

    #[test]
    fn hover_shows_convention_name_and_body() {
        let mut engine = engine_with_conventions();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"signed\"\n".to_owned(), 1),
        );
        let hover = engine.hover(&build, Position::new(0, 8)).expect("hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markup hover");
        };
        assert!(markup.value.contains("convention `signed`"));
        assert!(markup.value.contains("minifyEnabled"));
        assert!(markup.value.contains("```ulb"));
    }

    #[test]
    fn goto_definition_points_at_convention_name_in_conventions_ulb() {
        let mut engine = engine_with_conventions();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"signed\"\n".to_owned(), 1),
        );
        let GotoDefinitionResponse::Scalar(location) = engine
            .goto_definition(&build, Position::new(0, 8))
            .expect("goto")
        else {
            panic!("expected scalar location");
        };
        assert!(location.uri.path().ends_with("conventions.ulb"));
        let conventions_text = CONVENTIONS;
        let offset = conventions_text.find("signed").expect("convention name");
        let expected = span_to_range(
            conventions_text,
            Span {
                start: offset as u32,
                end: offset as u32 + 6,
            },
        );
        assert_eq!(location.range, expected);
    }

    #[test]
    fn goto_definition_returns_none_for_unknown_convention() {
        let mut engine = engine_with_conventions();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"ghost\"\n".to_owned(), 1),
        );
        assert!(
            engine
                .goto_definition(&build, Position::new(0, 8))
                .is_none()
        );
    }

    #[test]
    fn hover_outside_apply_is_none() {
        let mut engine = engine_with_conventions();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"signed\"\n".to_owned(), 1),
        );
        assert!(engine.hover(&build, Position::new(0, 0)).is_none());
    }

    #[test]
    fn open_conventions_document_shadows_disk_for_goto() {
        let mut engine = DiagnosticEngine::with_loader(MapLoader(HashMap::new()));
        let conventions = url("/proj/conventions.ulb");
        engine.upsert(
            conventions.clone(),
            Document::new("convention openDef {\n  compileSdk 30\n}\n".to_owned(), 1),
        );
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"openDef\"\n".to_owned(), 1),
        );
        let response = engine
            .goto_definition(&build, Position::new(0, 9))
            .expect("goto");
        let GotoDefinitionResponse::Scalar(location) = response else {
            panic!("expected scalar location");
        };
        assert_eq!(location.uri, conventions);
    }
}
