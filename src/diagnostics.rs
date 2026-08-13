//! The analysis engine: parses open documents and maps `ulb-lang`
//! diagnostics (plus evaluator diagnostics and convention-resolution
//! checks) onto the LSP protocol.
//!
//! Everything here is synchronous and independent of the LSP runtime so
//! tests can drive it directly; the server in `main.rs` is a thin adapter
//! that calls [`DiagnosticEngine::diagnostics_for`] and publishes.

use std::collections::BTreeMap;
use std::path::Path;

use lsp_types::{Diagnostic, DiagnosticSeverity, Range, Url};
use ulb_lang::ast::{ElseBranch, IfKind, Statement, StatementKind};
use ulb_lang::diagnostic::Severity;
use ulb_lang::eval::{Definitions, collect_definitions_lint, evaluate_build_lint};
use ulb_lang::parser::{Parsed, parse};

use crate::document::{Document, DocumentStore};
use crate::role::{Role, role_of};
use crate::utf16::span_to_range;

/// Reads role files that are not currently open in the editor.
///
/// The convention table for a `build.ulb` lives in the adjacent
/// `conventions.ulb`. When that file is not open, the engine asks a loader
/// for its text so `apply` checks still work against a fresh checkout.
pub trait SourceLoader {
    /// Returns the text of the file at `path`, or `None` if it cannot be
    /// read (missing file, permission denied, non-UTF-8 content).
    fn load(&self, path: &Path) -> Option<String>;
}

/// A [`SourceLoader`] that reads the local filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskLoader;

impl SourceLoader for DiskLoader {
    fn load(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// Turns open documents and role files into LSP diagnostics.
///
/// For every open document: parse it and report the lexer/parser
/// diagnostics. For a `build.ulb` additionally: resolve the adjacent
/// `conventions.ulb` and `libs.ulb` (an open document wins over the
/// loader), evaluate the document in lint mode and report its diagnostics
/// (unknown references/functions, arity and type errors, role violations),
/// and separately walk the AST to report `apply "name"` statements whose
/// convention is not defined — in both branches of an `if`, independent of
/// the evaluator's control flow.
#[derive(Debug)]
pub struct DiagnosticEngine<L> {
    store: DocumentStore,
    loader: L,
}

impl DiagnosticEngine<DiskLoader> {
    /// An engine that reads missing role files from the local filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::with_loader(DiskLoader)
    }
}

impl<L: SourceLoader> DiagnosticEngine<L> {
    /// An engine that resolves missing role files through `loader`.
    #[must_use]
    pub fn with_loader(loader: L) -> Self {
        Self {
            store: DocumentStore::new(),
            loader,
        }
    }

    /// Inserts or replaces the open document at `uri`.
    pub fn upsert(&mut self, uri: Url, document: Document) {
        self.store.upsert(uri, document);
    }

    /// Applies an incremental change to the open document at `uri`,
    /// updating its version. Returns whether a document was present and
    /// updated.
    pub fn apply_change(
        &mut self,
        uri: &Url,
        version: i32,
        range: Option<Range>,
        replacement: String,
    ) -> bool {
        self.store.apply_change(uri, version, range, replacement)
    }

    /// Closes the document at `uri`.
    pub fn close(&mut self, uri: &Url) {
        self.store.remove(uri);
    }

    /// The URIs of all open documents, sorted for deterministic iteration.
    #[must_use]
    pub fn documents(&self) -> Vec<(Url, &Document)> {
        let mut uris: Vec<(Url, &Document)> = self
            .store
            .iter()
            .map(|(uri, doc)| (uri.clone(), doc))
            .collect();
        uris.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        uris
    }

    /// The URIs of all open `build.ulb` documents, sorted. Used to re-run
    /// diagnostics when the convention table changes.
    #[must_use]
    pub fn build_uris(&self) -> Vec<Url> {
        self.documents()
            .into_iter()
            .filter(|(uri, _)| role_of(uri) == Role::Build)
            .map(|(uri, _)| uri)
            .collect()
    }

    /// Diagnostics for the document at `uri`, or an empty list if it is
    /// not open.
    ///
    /// # Examples
    ///
    /// ```
    /// use lsp_types::Url;
    /// use ulb_lsp::diagnostics::{DiagnosticEngine, SourceLoader};
    /// use ulb_lsp::document::Document;
    /// use std::path::Path;
    ///
    /// let mut engine = DiagnosticEngine::with_loader(EmptyLoader);
    /// let url = Url::parse("file:///proj/build.ulb").expect("valid URL");
    /// engine.upsert(
    ///     url.clone(),
    ///     Document::new(r#"apply "missing""#.to_owned(), 1),
    /// );
    /// let diagnostics = engine.diagnostics_for(&url);
    /// assert_eq!(diagnostics.len(), 1);
    /// assert_eq!(diagnostics[0].message, "unknown convention 'missing'");
    ///
    /// struct EmptyLoader;
    /// impl SourceLoader for EmptyLoader {
    ///     fn load(&self, _path: &Path) -> Option<String> { None }
    /// }
    /// ```
    #[must_use]
    pub fn diagnostics_for(&self, uri: &Url) -> Vec<Diagnostic> {
        let Some(document) = self.store.get(uri) else {
            return Vec::new();
        };
        let parsed = parse(&document.text);
        let mut out = parse_diagnostics(&document.text, &parsed);
        if role_of(uri) == Role::Build {
            out.extend(self.unknown_convention_diagnostics(uri, document, &parsed));
            out.extend(self.evaluation_diagnostics(uri, &document.text, &parsed));
        }
        out
    }

    /// Runs the evaluator in lint mode over the document's AST and maps its
    /// diagnostics onto the protocol. Lint mode never consults the process
    /// or filesystem, so `env`/`props` lookups do not produce spurious
    /// errors. The evaluator's own "unknown convention" report is dropped
    /// here — the targeted AST walk
    /// ([`Self::unknown_convention_diagnostics`]) owns that check so that
    /// applies inside conditionals are verified in both branches.
    fn evaluation_diagnostics(&self, uri: &Url, text: &str, parsed: &Parsed) -> Vec<Diagnostic> {
        let defs = self.resolve_definitions(uri);
        let outcome = evaluate_build_lint(&parsed.file, &defs);
        outcome
            .diagnostics
            .iter()
            .filter(|d| !d.message.starts_with("unknown convention '"))
            .map(|d| Diagnostic {
                range: span_to_range(text, d.span),
                severity: Some(match d.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                    Severity::Info => DiagnosticSeverity::INFORMATION,
                }),
                source: Some("ulb-lang".to_owned()),
                message: d.message.clone(),
                ..Default::default()
            })
            .collect()
    }

    /// Reports `apply "name"` statements whose convention is not defined
    /// in the adjacent `conventions.ulb`. Walks nested blocks and
    /// `if`/`else` bodies so applies inside conditionals are checked too.
    fn unknown_convention_diagnostics(
        &self,
        uri: &Url,
        document: &Document,
        parsed: &Parsed,
    ) -> Vec<Diagnostic> {
        let conventions = self.convention_names(uri);
        let mut out = Vec::new();
        for statement in &parsed.file.statements {
            collect_applies(statement, &conventions, &document.text, &mut out);
        }
        out
    }

    /// The set of convention names defined by the `conventions.ulb` next to
    /// `uri`, resolved from the open document or the loader. Empty when the
    /// file cannot be found at all, which makes every `apply` unknown.
    fn convention_names(&self, uri: &Url) -> BTreeMap<String, ()> {
        self.resolve_definitions(uri)
            .conventions
            .keys()
            .map(|name| (name.clone(), ()))
            .collect()
    }

    /// Collects the definitions a `build.ulb` at `uri` sees: the adjacent
    /// `conventions.ulb` and `libs.ulb`, each resolved from the open
    /// document or the loader, collected in lint mode so `env`/`props`
    /// inside them never touch the process or filesystem. Both files are
    /// globally visible to every `build.ulb` (GRAMMAR.md §6.3/§6.4), which
    /// is why name resolution must see them together.
    fn resolve_definitions(&self, uri: &Url) -> Definitions {
        let mut defs = Definitions::default();
        for file_name in ["conventions.ulb", "libs.ulb"] {
            let Some(role_url) = uri.join(file_name).ok() else {
                continue;
            };
            let Some(text) = self.text_of(&role_url) else {
                continue;
            };
            let parsed = parse(&text);
            let mut diagnostics = Vec::new();
            // Diagnostics about the role file itself are dropped here; they
            // will surface when that file is analyzed in its own right.
            collect_definitions_lint(&parsed.file, &mut defs, &mut diagnostics);
        }
        defs
    }

    /// The text at `uri`: the open document if there is one, otherwise the
    /// loader for the underlying file path.
    pub(crate) fn text_of(&self, uri: &Url) -> Option<String> {
        if let Some(document) = self.store.get(uri) {
            return Some(document.text.clone());
        }
        let path = uri.to_file_path().ok()?;
        self.loader.load(&path)
    }
}

impl Default for DiagnosticEngine<DiskLoader> {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps every lexer/parser diagnostic from a parse onto the protocol,
/// preserving the source span and severity.
fn parse_diagnostics(text: &str, parsed: &Parsed) -> Vec<Diagnostic> {
    parsed
        .diagnostics
        .iter()
        .map(|diag| Diagnostic {
            range: span_to_range(text, diag.span),
            severity: Some(match diag.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
                Severity::Info => DiagnosticSeverity::INFORMATION,
            }),
            source: Some("ulb-lang".to_owned()),
            message: diag.message.clone(),
            ..Default::default()
        })
        .collect()
}

/// Recursively walks one statement's tree collecting `apply` statements
/// that reference an undefined convention.
fn collect_applies(
    statement: &Statement,
    conventions: &BTreeMap<String, ()>,
    text: &str,
    out: &mut Vec<Diagnostic>,
) {
    match &statement.kind {
        StatementKind::Apply { name, .. } if !conventions.contains_key(name) => {
            out.push(Diagnostic {
                range: span_to_range(text, statement.span),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("ulb-lang".to_owned()),
                message: format!("unknown convention '{name}'"),
                ..Default::default()
            });
        }
        StatementKind::BlockStmt { block, .. } => {
            for child in &block.statements {
                collect_applies(child, conventions, text, out);
            }
        }
        StatementKind::If(if_kind) => collect_applies_if(if_kind, conventions, text, out),
        _ => {}
    }
}

fn collect_applies_if(
    if_kind: &IfKind,
    conventions: &BTreeMap<String, ()>,
    text: &str,
    out: &mut Vec<Diagnostic>,
) {
    for child in &if_kind.then_branch.statements {
        collect_applies(child, conventions, text, out);
    }
    match &if_kind.else_branch {
        None => {}
        Some(ElseBranch::Block(block)) => {
            for child in &block.statements {
                collect_applies(child, conventions, text, out);
            }
        }
        Some(ElseBranch::If(inner)) => collect_applies_if(&inner.kind, conventions, text, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct MapLoader(HashMap<PathBuf, String>);

    impl SourceLoader for MapLoader {
        fn load(&self, path: &Path) -> Option<String> {
            self.0.get(path).cloned()
        }
    }

    fn url(path: &str) -> Url {
        Url::from_file_path(path).expect("absolute file path")
    }

    const CONVENTIONS: &str = r#"
convention androidApp {
  compileSdk 37
}
convention signed {
  minifyEnabled true
}
"#;

    fn loader_with_conventions(build_path: &str) -> MapLoader {
        let mut map = HashMap::new();
        let dir = Path::new(build_path).parent().expect("parent dir");
        map.insert(dir.join("conventions.ulb"), CONVENTIONS.to_owned());
        MapLoader(map)
    }

    #[test]
    fn parse_errors_are_reported_as_errors() {
        let mut engine = DiagnosticEngine::new();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"unterminated\ncompileSdk 37".to_owned(), 1),
        );
        let diagnostics = engine.diagnostics_for(&build);
        assert!(!diagnostics.is_empty());
        assert!(
            diagnostics
                .iter()
                .all(|d| d.severity == Some(DiagnosticSeverity::ERROR))
        );
    }

    #[test]
    fn known_and_unknown_apply_resolution() {
        let mut engine = DiagnosticEngine::with_loader(loader_with_conventions("/proj/build.ulb"));
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new(
                "apply \"androidApp\"\napply \"signed\"\napply \"ghost\"\n".to_owned(),
                1,
            ),
        );
        let diagnostics = engine.diagnostics_for(&build);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unknown convention 'ghost'");
        let range = diagnostics[0].range;
        assert_eq!(range.start.line, 2);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 2);
        assert_eq!(range.end.character, 13);
    }

    #[test]
    fn open_conventions_document_shadows_disk() {
        let mut engine = DiagnosticEngine::with_loader(MapLoader(HashMap::new()));
        let conventions = url("/proj/conventions.ulb");
        engine.upsert(
            conventions,
            Document::new("convention local { }".to_owned(), 1),
        );
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("apply \"local\"".to_owned(), 1),
        );
        assert!(engine.diagnostics_for(&build).is_empty());
    }

    #[test]
    fn unknown_alias_is_reported() {
        let mut engine = DiagnosticEngine::with_loader(loader_with_conventions("/proj/build.ulb"));
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("deps {\n  implementation ghostAlias\n}\n".to_owned(), 1),
        );
        let diagnostics = engine.diagnostics_for(&build);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unknown reference 'ghostAlias'");
    }

    #[test]
    fn libs_aliases_resolve_cleanly() {
        let mut map = HashMap::new();
        map.insert(
            Path::new("/proj").join("libs.ulb"),
            "coreKtx = \"androidx.core:core-ktx:1.16.0\"\n".to_owned(),
        );
        let mut engine = DiagnosticEngine::with_loader(MapLoader(map));
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("deps {\n  implementation coreKtx\n}\n".to_owned(), 1),
        );
        assert!(engine.diagnostics_for(&build).is_empty());
    }

    #[test]
    fn unknown_function_call_is_reported() {
        let mut engine = DiagnosticEngine::with_loader(loader_with_conventions("/proj/build.ulb"));
        let build = url("/proj/build.ulb");
        engine.upsert(build.clone(), Document::new("defaultDebug()".to_owned(), 1));
        let diagnostics = engine.diagnostics_for(&build);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unknown function 'defaultDebug'");
    }

    #[test]
    fn convention_definition_inside_build_is_a_role_violation() {
        let mut engine = DiagnosticEngine::new();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new("convention nope {\n  compileSdk 37\n}\n".to_owned(), 1),
        );
        let diagnostics = engine.diagnostics_for(&build);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("only valid in conventions.ulb"))
        );
    }

    #[test]
    fn env_and_props_are_unresolved_not_errors() {
        let mut engine = DiagnosticEngine::new();
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new(
                "x env(\"ULB_LSP_UNSET_VAR\")\ny props(\"/absent/ulb.properties\").key\n"
                    .to_owned(),
                1,
            ),
        );
        assert!(engine.diagnostics_for(&build).is_empty());
    }

    #[test]
    fn applies_inside_blocks_and_conditionals_are_scanned() {
        let mut engine = DiagnosticEngine::with_loader(loader_with_conventions("/proj/build.ulb"));
        let build = url("/proj/build.ulb");
        engine.upsert(
            build.clone(),
            Document::new(
                r#"android {
  apply "ghost-in-block"
}
if debugBuild {
  apply "ghost-in-then"
} else {
  apply "ghost-in-else"
}
"#
                .to_owned(),
                1,
            ),
        );
        let diagnostics = engine.diagnostics_for(&build);
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "unknown convention 'ghost-in-block'",
                "unknown convention 'ghost-in-then'",
                "unknown convention 'ghost-in-else'",
                "unknown reference 'debugBuild'"
            ]
        );
    }

    #[test]
    fn non_build_roles_do_not_scan_applies() {
        let mut engine = DiagnosticEngine::new();
        for path in [
            "/proj/settings.ulb",
            "/proj/libs.ulb",
            "/proj/conventions.ulb",
        ] {
            let uri = url(path);
            engine.upsert(uri.clone(), Document::new("apply \"nope\"".to_owned(), 1));
            let diagnostics = engine.diagnostics_for(&uri);
            let unknown: Vec<&Diagnostic> = diagnostics
                .iter()
                .filter(|d| d.message.starts_with("unknown convention"))
                .collect();
            assert!(unknown.is_empty(), "{path} must not flag applies");
        }
    }

    #[test]
    fn diagnostics_for_unopened_document_is_empty() {
        let engine = DiagnosticEngine::<DiskLoader>::new();
        assert!(engine.diagnostics_for(&url("/proj/build.ulb")).is_empty());
    }

    #[test]
    fn build_uris_lists_only_build_role() {
        let mut engine = DiagnosticEngine::new();
        engine.upsert(url("/proj/build.ulb"), Document::new("".to_owned(), 1));
        engine.upsert(url("/proj/libs.ulb"), Document::new("".to_owned(), 1));
        let builds = engine.build_uris();
        assert_eq!(builds.len(), 1);
        assert!(builds[0].path().ends_with("build.ulb"));
    }
}
