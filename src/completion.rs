//! `textDocument/completion` and plugin-owned hover for `build.ulb`
//! (ARCHITECTURE.md §11.1).
//!
//! The engine resolves the plugins a `build.ulb` applies (via the adjacent
//! `libs.ulb` `plugins {}` table, collected in lint mode) and reads each
//! plugin's cached `.wasm` from the plugin cache directory, extracting the
//! embedded config schema with `ulb-schema`. Inside a plugin-owned block the
//! cursor is offered that block's schema keys; outside, the hardcoded tool
//! vocabulary for the enclosing core block (`deps` scopes, `run` actions,
//! task body, or the top level).
//!
//! Schema extraction is degraded by design: a plugin whose cached artifact
//! is missing, not yet downloaded, or predates the schema custom section
//! contributes nothing (its blocks simply fall back to the core vocabulary)
//! rather than failing the request.

use std::collections::BTreeMap;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Hover, HoverContents, MarkupContent,
    MarkupKind, Position, Url,
};
use ulb_lang::parser::parse;
use ulb_schema::{PluginSchema, extract_schema, plugin::PluginSpec};

use crate::diagnostics::{DiagnosticEngine, SourceLoader};
use crate::plugin_schema::{CoreBlock, Scope, innermost_scope, key_at, properties_completions};
use crate::role::{Role, role_of};
use crate::utf16::{position_to_offset, span_to_range};

impl<L: SourceLoader> DiagnosticEngine<L> {
    /// Completion items for the position in the document at `uri`.
    ///
    /// For a `build.ulb`: resolves the applied plugins' config schemas and
    /// offers the keys valid inside the enclosing block (see the module
    /// docs). For any other role (and for unresolvable positions) returns
    /// `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use std::path::{Path, PathBuf};
    /// use lsp_types::{CompletionResponse, Position, Url};
    /// use ulb_lsp::diagnostics::{DiagnosticEngine, SourceLoader};
    /// use ulb_lsp::document::Document;
    ///
    /// struct Loader {
    ///     text: HashMap<PathBuf, String>,
    ///     bytes: HashMap<PathBuf, Vec<u8>>,
    /// }
    /// impl SourceLoader for Loader {
    ///     fn load(&self, path: &Path) -> Option<String> {
    ///         self.text.get(path).cloned()
    ///     }
    ///     fn load_bytes(&self, path: &Path) -> Option<Vec<u8>> {
    ///         self.bytes.get(path).cloned()
    ///     }
    /// }
    ///
    /// // A minimal wasm module with one `ulb-config-schema` custom section.
    /// fn leb128(mut value: usize) -> Vec<u8> {
    ///     let mut out = Vec::new();
    ///     loop {
    ///         let mut byte = (value & 0x7f) as u8;
    ///         value >>= 7;
    ///         if value != 0 { byte |= 0x80; }
    ///         out.push(byte);
    ///         if value == 0 { break; }
    ///     }
    ///     out
    /// }
    /// fn wasm_with_schema(schema: &str) -> Vec<u8> {
    ///     let mut w = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    ///     let name = b"ulb-config-schema";
    ///     let mut payload = leb128(name.len());
    ///     payload.extend_from_slice(name);
    ///     payload.extend_from_slice(schema.as_bytes());
    ///     w.push(0x00); // custom section id
    ///     w.extend(leb128(payload.len()));
    ///     w.extend(payload);
    ///     w
    /// }
    ///
    /// let schema = r#"{"name":"ulite/android","properties":[{"name":"android","type_name":"object","description":"Android configuration.","required":true,"properties":[{"name":"compileSdk","type_name":"integer","description":"The compile SDK version.","required":true,"properties":[]}]}]}"#;
    /// let mut text = HashMap::new();
    /// text.insert(
    ///     PathBuf::from("/proj/libs.ulb"),
    ///     "versions { ulbAndroid = \"0.1.0\" }\nplugins { android = \"ulite/android\" @ ulbAndroid }\n"
    ///         .to_owned(),
    /// );
    /// let mut bytes = HashMap::new();
    /// bytes.insert(
    ///     PathBuf::from("/cache/ulite/android/0.1.0/plugin.wasm"),
    ///     wasm_with_schema(schema),
    /// );
    /// let mut engine = DiagnosticEngine::with_loader(Loader { text, bytes });
    /// engine.set_plugin_cache_dir(PathBuf::from("/cache"));
    /// let build = Url::from_file_path("/proj/build.ulb").expect("file URL");
    /// engine.upsert(
    ///     build.clone(),
    ///     Document::new("android {\n  comp\n}\n".to_owned(), 1),
    /// );
    ///
    /// let response = engine
    ///     .completion(&build, Position::new(1, 5))
    ///     .expect("completion");
    /// let CompletionResponse::Array(items) = response else {
    ///     panic!("expected array of items");
    /// };
    /// assert!(items.iter().any(|i| i.label == "compileSdk"));
    /// ```
    #[must_use]
    pub fn completion(&self, uri: &Url, position: Position) -> Option<CompletionResponse> {
        if role_of(uri) != Role::Build {
            return None;
        }
        let text = self.text_of(uri)?;
        let offset = position_to_offset(&text, position)?;
        let schemas: Vec<PluginSchema> = self.resolve_plugin_schemas(uri).into_values().collect();
        let parsed = parse(&text);
        let scope = innermost_scope(&parsed.file.statements, offset, &schemas);
        let items = match scope {
            Scope::Plugin { field } => properties_completions(&field.properties),
            Scope::Core { block } => {
                let mut items = core_completions(block);
                if block == CoreBlock::Top {
                    for schema in &schemas {
                        items.extend(properties_completions(&schema.properties));
                    }
                }
                items
            }
        };
        Some(CompletionResponse::Array(items))
    }

    /// Hover content for a plugin-owned key in a `build.ulb`: the schema
    /// field's type and description. Returns `None` when the cursor is not
    /// on a key declared by an applied plugin's config schema.
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
    /// struct Loader {
    ///     text: HashMap<PathBuf, String>,
    ///     bytes: HashMap<PathBuf, Vec<u8>>,
    /// }
    /// impl SourceLoader for Loader {
    ///     fn load(&self, path: &Path) -> Option<String> {
    ///         self.text.get(path).cloned()
    ///     }
    ///     fn load_bytes(&self, path: &Path) -> Option<Vec<u8>> {
    ///         self.bytes.get(path).cloned()
    ///     }
    /// }
    ///
    /// fn leb128(mut value: usize) -> Vec<u8> {
    ///     let mut out = Vec::new();
    ///     loop {
    ///         let mut byte = (value & 0x7f) as u8;
    ///         value >>= 7;
    ///         if value != 0 { byte |= 0x80; }
    ///         out.push(byte);
    ///         if value == 0 { break; }
    ///     }
    ///     out
    /// }
    /// fn wasm_with_schema(schema: &str) -> Vec<u8> {
    ///     let mut w = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    ///     let name = b"ulb-config-schema";
    ///     let mut payload = leb128(name.len());
    ///     payload.extend_from_slice(name);
    ///     payload.extend_from_slice(schema.as_bytes());
    ///     w.push(0x00);
    ///     w.extend(leb128(payload.len()));
    ///     w.extend(payload);
    ///     w
    /// }
    ///
    /// let schema = r#"{"name":"ulite/android","properties":[{"name":"android","type_name":"object","description":"Android configuration.","required":true,"properties":[{"name":"compileSdk","type_name":"integer","description":"The compile SDK version.","required":true,"properties":[]}]}]}"#;
    /// let mut text = HashMap::new();
    /// text.insert(
    ///     PathBuf::from("/proj/libs.ulb"),
    ///     "versions { ulbAndroid = \"0.1.0\" }\nplugins { android = \"ulite/android\" @ ulbAndroid }\n"
    ///         .to_owned(),
    /// );
    /// let mut bytes = HashMap::new();
    /// bytes.insert(
    ///     PathBuf::from("/cache/ulite/android/0.1.0/plugin.wasm"),
    ///     wasm_with_schema(schema),
    /// );
    /// let mut engine = DiagnosticEngine::with_loader(Loader { text, bytes });
    /// engine.set_plugin_cache_dir(PathBuf::from("/cache"));
    /// let build = Url::from_file_path("/proj/build.ulb").expect("file URL");
    /// engine.upsert(
    ///     build.clone(),
    ///     Document::new("android {\n  compileSdk 37\n}\n".to_owned(), 1),
    /// );
    ///
    /// let hover = engine
    ///     .plugin_field_hover(&build, Position::new(1, 3))
    ///     .expect("hover");
    /// let HoverContents::Markup(markup) = hover.contents else {
    ///     panic!("expected markup hover");
    /// };
    /// assert_eq!(markup.kind, MarkupKind::Markdown);
    /// assert!(markup.value.contains("The compile SDK version."));
    /// ```
    #[must_use]
    pub fn plugin_field_hover(&self, uri: &Url, position: Position) -> Option<Hover> {
        if role_of(uri) != Role::Build {
            return None;
        }
        let text = self.text_of(uri)?;
        let offset = position_to_offset(&text, position)?;
        let schemas: Vec<PluginSchema> = self.resolve_plugin_schemas(uri).into_values().collect();
        let parsed = parse(&text);
        let scope = innermost_scope(&parsed.file.statements, offset, &schemas);
        let Scope::Plugin { field } = scope else {
            return None;
        };
        let key = key_at(&parsed.file.statements, offset)?;
        let prop = field.properties.iter().find(|p| p.name == key.text)?;
        let key_range = span_to_range(&text, key.span);
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_value(prop),
            }),
            range: Some(key_range),
        })
    }

    /// Resolves each plugin a `build.ulb` applies to its extracted config
    /// schema, read from the plugin cache directory. Plugins whose value is
    /// not a pin-able coordinate, whose artifact is not cached, or whose
    /// `.wasm` carries no schema section are skipped.
    fn resolve_plugin_schemas(&self, uri: &Url) -> BTreeMap<String, PluginSchema> {
        let defs = self.resolve_definitions(uri);
        let cache_dir = self.plugin_cache_dir();
        let mut out = BTreeMap::new();
        for (alias, value) in &defs.plugins {
            let Ok(spec) = PluginSpec::from_value(value) else {
                continue;
            };
            let Some(wasm_path) = spec.cache_wasm_path(&cache_dir) else {
                continue;
            };
            let schema = match self.schema_cached(&wasm_path) {
                Some(cached) => cached,
                None => {
                    let extracted = self
                        .load_bytes(&wasm_path)
                        .and_then(|bytes| extract_schema(&bytes));
                    self.cache_schema(wasm_path.clone(), extracted.clone());
                    extracted
                }
            };
            if let Some(schema) = schema {
                out.insert(alias.clone(), schema);
            }
        }
        out
    }
}

/// The hover body for schema field `prop`.
fn hover_value(prop: &ulb_schema::SchemaField) -> String {
    let type_line = match prop.type_name.as_str() {
        "object" if !prop.properties.is_empty() => "block",
        "object" => "object",
        "array" => prop.items.as_deref().unwrap_or("array"),
        other => other,
    };
    let required = if prop.required { " (required)" } else { "" };
    let mut value = format!("**`{}`** — {type_line}{required}", prop.name);
    if !prop.description.is_empty() {
        value.push_str("\n\n");
        value.push_str(&prop.description);
    }
    value
}

/// The hardcoded tool vocabulary for a non-plugin block context. This is
/// the core `uliab`/`ulb-lang` statement surface for a `build.ulb`
/// (GRAMMAR.md §6.2) — it never changes with the plugins applied to a
/// project.
fn core_completions(block: CoreBlock) -> Vec<CompletionItem> {
    match block {
        CoreBlock::Deps => deps_scope_items(),
        CoreBlock::Run => run_action_items(),
        CoreBlock::Task => task_body_items(),
        CoreBlock::Top => top_level_items(),
    }
}

fn keyword(label: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some(detail.to_owned()),
        ..Default::default()
    }
}

fn statement_item(label: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some(detail.to_owned()),
        ..Default::default()
    }
}

fn top_level_items() -> Vec<CompletionItem> {
    vec![
        statement_item("plugin", "plugin \"alias\" — apply a plugin from libs.ulb"),
        statement_item(
            "apply",
            "apply \"name\" — apply a convention from conventions.ulb",
        ),
        statement_item("deps", "deps { } — module dependencies"),
        statement_item("task", "task \"name\" { } — custom task"),
        statement_item("if", "if cond { } [else { }] — conditional configuration"),
        statement_item("else", "else { } — else branch of an if"),
        statement_item("description", "description \"...\" — module description"),
    ]
}

fn deps_scope_items() -> Vec<CompletionItem> {
    const SCOPES: [(&str, &str); 7] = [
        ("api", "transitive to consumers"),
        ("implementation", "module-private"),
        ("runtimeOnly", "runtime only, not compile"),
        ("compileOnly", "compile only, not runtime"),
        ("ksp", "annotation-processor classpath (KSP step)"),
        ("testImplementation", "test compilation/runtime"),
        (
            "androidTestImplementation",
            "androidTest compilation/runtime",
        ),
    ];
    SCOPES
        .into_iter()
        .map(|(name, detail)| keyword(name, detail))
        .collect()
}

fn task_body_items() -> Vec<CompletionItem> {
    vec![
        statement_item("description", "description \"...\" — task description"),
        statement_item(
            "dependsOn",
            "dependsOn [ \"task1\", ... ] — task dependencies",
        ),
        statement_item("run", "run { } — the task's action block"),
    ]
}

fn run_action_items() -> Vec<CompletionItem> {
    vec![
        statement_item(
            "exec",
            "exec(command=\"...\", args=[...]) — run an allowlisted command",
        ),
        statement_item("copy", "copy(from=\"...\", to=\"...\") — copy a directory"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    /// A valid `ulite/android` plugin schema whose only top-level field is
    /// the `android` object block.
    const ANDROID_SCHEMA: &str = r#"{"name":"ulite/android","properties":[{"name":"android","type_name":"object","description":"Android configuration.","required":true,"properties":[{"name":"compileSdk","type_name":"integer","description":"The compile SDK version.","required":true,"properties":[]}]}]}"#;

    fn leb128(mut value: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
        out
    }

    /// A minimal wasm module whose only custom section embeds `schema`.
    fn wasm_with_schema(schema: &[u8]) -> Vec<u8> {
        let mut w = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let name = b"ulb-config-schema";
        let mut payload = leb128(name.len());
        payload.extend_from_slice(name);
        payload.extend_from_slice(schema);
        w.push(0x00);
        w.extend(leb128(payload.len()));
        w.extend(payload);
        w
    }

    /// An in-memory loader that counts `.wasm` reads, so schema caching can
    /// be asserted.
    struct CountingLoader {
        text: HashMap<PathBuf, String>,
        bytes: HashMap<PathBuf, Vec<u8>>,
        reads: Rc<Cell<usize>>,
    }

    impl SourceLoader for CountingLoader {
        fn load(&self, path: &Path) -> Option<String> {
            self.text.get(path).cloned()
        }
        fn load_bytes(&self, path: &Path) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.bytes.get(path).cloned()
        }
    }

    /// An engine with the `android` plugin applied and its `.wasm` cached
    /// under `/cache`; `reads` counts `load_bytes` calls.
    fn engine_with_android(reads: Rc<Cell<usize>>) -> DiagnosticEngine<CountingLoader> {
        let mut text = HashMap::new();
        text.insert(
            PathBuf::from("/proj/libs.ulb"),
            "versions { v = \"0.1.0\" }\nplugins { android = \"ulite/android\" @ v }\n".to_owned(),
        );
        let mut bytes = HashMap::new();
        bytes.insert(
            PathBuf::from("/cache/ulite/android/0.1.0/plugin.wasm"),
            wasm_with_schema(ANDROID_SCHEMA.as_bytes()),
        );
        let mut engine = DiagnosticEngine::with_loader(CountingLoader { text, bytes, reads });
        engine.set_plugin_cache_dir(PathBuf::from("/cache"));
        engine
    }

    fn build_uri() -> Url {
        Url::from_file_path("/proj/build.ulb").expect("absolute path")
    }

    fn labels_of(
        engine: &mut DiagnosticEngine<CountingLoader>,
        uri: &Url,
        text: &str,
    ) -> Vec<String> {
        engine.upsert(uri.clone(), Document::new(text.to_owned(), 1));
        let response = engine
            .completion(uri, Position::new(1, 5))
            .expect("completion for open build");
        let CompletionResponse::Array(items) = response else {
            panic!("expected array of items");
        };
        items.into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn inside_plugin_block_offers_the_schema_key() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads);
        let labels = labels_of(&mut engine, &build_uri(), "android {\n  comp\n}\n");
        assert!(labels.iter().any(|l| l == "compileSdk"));
    }

    #[test]
    fn top_level_offers_core_and_plugin_block_name() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads);
        let uri = build_uri();
        engine.upsert(
            uri.clone(),
            Document::new("plugin \"android\"\nandroid {\n}\n".to_owned(), 1),
        );
        let response = engine
            .completion(&uri, Position::new(0, 0))
            .expect("completion");
        let CompletionResponse::Array(items) = response else {
            panic!("expected array");
        };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"deps"));
        assert!(labels.contains(&"apply"));
        // The applied plugin's top-level block name completes too.
        assert!(labels.contains(&"android"));
    }

    #[test]
    fn inside_deps_offers_only_dependency_scopes() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads);
        let labels = labels_of(&mut engine, &build_uri(), "deps {\n  imp\n}\n");
        assert!(labels.iter().any(|l| l == "implementation"));
        // `apply` is a top-level statement, not a deps scope.
        assert!(!labels.iter().any(|l| l == "apply"));
    }

    #[test]
    fn inside_run_offers_task_actions() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads);
        let uri = build_uri();
        engine.upsert(
            uri.clone(),
            Document::new("task \"a\" {\n  run {\n    ex\n  }\n}\n".to_owned(), 1),
        );
        let response = engine
            .completion(&uri, Position::new(2, 5))
            .expect("completion");
        let CompletionResponse::Array(items) = response else {
            panic!("expected array");
        };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"exec"));
        assert!(labels.contains(&"copy"));
    }

    #[test]
    fn non_build_role_yields_no_completion() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads);
        let settings = Url::from_file_path("/proj/settings.ulb").expect("file URL");
        engine.upsert(
            settings.clone(),
            Document::new("project \"A\"\n".to_owned(), 1),
        );
        assert!(engine.completion(&settings, Position::new(0, 0)).is_none());
    }

    #[test]
    fn hover_none_for_key_not_in_schema() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads);
        let uri = build_uri();
        engine.upsert(
            uri.clone(),
            Document::new("android {\n  versionCode 7\n}\n".to_owned(), 1),
        );
        assert!(
            engine
                .plugin_field_hover(&uri, Position::new(1, 3))
                .is_none()
        );
    }

    #[test]
    fn schema_is_cached_after_first_request() {
        let reads = Rc::new(Cell::new(0));
        let mut engine = engine_with_android(reads.clone());
        let uri = build_uri();
        engine.upsert(
            uri.clone(),
            Document::new("android {\n  comp\n}\n".to_owned(), 1),
        );
        // The plugin .wasm is read once, on the first request, then cached.
        engine.completion(&uri, Position::new(1, 5)).expect("first");
        assert_eq!(reads.get(), 1);
        engine
            .completion(&uri, Position::new(1, 5))
            .expect("second");
        assert_eq!(reads.get(), 1, "second request must hit the cache");
    }

    #[test]
    fn missing_wasm_degrades_gracefully() {
        let reads = Rc::new(Cell::new(0));
        let mut text = HashMap::new();
        text.insert(
            PathBuf::from("/proj/libs.ulb"),
            "versions { v = \"0.1.0\" }\nplugins { android = \"ulite/android\" @ v }\n".to_owned(),
        );
        let mut engine = DiagnosticEngine::with_loader(CountingLoader {
            text,
            bytes: HashMap::new(),
            reads,
        });
        engine.set_plugin_cache_dir(PathBuf::from("/cache"));
        let uri = build_uri();
        engine.upsert(
            uri.clone(),
            Document::new("android {\n  comp\n}\n".to_owned(), 1),
        );
        // No plugin artifact cached: completion still answers with the core
        // vocabulary instead of failing.
        let response = engine
            .completion(&uri, Position::new(1, 5))
            .expect("completion despite missing artifact");
        let CompletionResponse::Array(items) = response else {
            panic!("expected array");
        };
        assert!(!items.is_empty(), "core vocabulary still offered");
    }
}
