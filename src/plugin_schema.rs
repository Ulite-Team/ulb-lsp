//! Driving editor completions and hover from a plugin's embedded config
//! schema (ARCHITECTURE.md §8 / §11.1).
//!
//! A `build.ulb` names plugins via `libs.ulb`'s `plugins {}` table; each
//! plugin's cached `.wasm` carries a `ulb-schema` config schema in a custom
//! section. This module turns those schemas into the context the editor
//! needs: which block a cursor is inside (and thus which keys are valid
//! there), and the [`CompletionItem`]s that surface those keys with their
//! documentation.
//!
//! Plugin-owned blocks are detected purely from the schema — a block whose
//! name matches an `"object"` schema field with declared nested properties
//! is plugin-owned; anything else falls back to the tool vocabulary
//! hardcoded in [`crate::completion`].
//!
//! The two pieces here are pure over `&[PluginSchema]` so they are
//! unit-testable without a real plugin artifact; the `.wasm` read and
//! schema extraction live on the engine in `completion.rs`.

use lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, InsertTextFormat, MarkupContent, MarkupKind,
};
use ulb_lang::ast::{ElseBranch, Ident, IfKind, Path, Statement, StatementKind};
use ulb_lang::span::Span;
use ulb_schema::{PluginSchema, SchemaField};

/// The core (non-plugin) block a cursor is inside, selecting which piece of
/// the hardcoded tool vocabulary to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoreBlock {
    /// Top level of a `build.ulb`, or any statement not inside a
    /// `deps`/`run` block.
    Top,
    /// Inside `deps { }` / `<sourceSet>.deps { }` — dependency scopes only.
    Deps,
    /// Inside a `task "name" { }` body (below the `run` block).
    Task,
    /// Inside a task's `run { }` — closed action set (`exec`, `copy`).
    Run,
}

/// The completion context resolved for a cursor position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Scope<'a> {
    /// Not inside a plugin-owned block: offer the core vocabulary.
    Core { block: CoreBlock },
    /// Inside the plugin-owned block opened by `field`; its `properties`
    /// are the keys valid at the cursor.
    Plugin { field: &'a SchemaField },
}

/// Resolves the innermost completion scope for the byte `offset` inside
/// `statements`, given the schemas of every plugin applied to the file.
///
/// Walks nested blocks, descending into the first statement that contains
/// `offset`. A block whose header matches an `"object"` schema field with
/// nested properties opens a [`Scope::Plugin`]; `deps`/`run`/`task` open
/// the matching [`CoreBlock`]; anything else is [`CoreBlock::Top`].
pub(crate) fn innermost_scope<'a>(
    statements: &[Statement],
    offset: u32,
    schemas: &'a [PluginSchema],
) -> Scope<'a> {
    walk(
        statements,
        offset,
        schemas,
        Scope::Core {
            block: CoreBlock::Top,
        },
    )
}

fn walk<'a>(
    statements: &[Statement],
    offset: u32,
    schemas: &'a [PluginSchema],
    scope: Scope<'a>,
) -> Scope<'a> {
    for stmt in statements {
        if contains(stmt.span, offset) {
            return inside(stmt, offset, schemas, scope);
        }
    }
    scope
}

fn inside<'a>(
    stmt: &Statement,
    offset: u32,
    schemas: &'a [PluginSchema],
    scope: Scope<'a>,
) -> Scope<'a> {
    match &stmt.kind {
        StatementKind::BlockStmt { path, block } => {
            let child = descend(path, scope, schemas);
            walk(&block.statements, offset, schemas, child)
        }
        StatementKind::ConventionDef { block, .. } | StatementKind::FnDef { block, .. } => {
            walk(&block.statements, offset, schemas, scope)
        }
        StatementKind::TaskDef { block, .. } => walk(
            &block.statements,
            offset,
            schemas,
            Scope::Core {
                block: CoreBlock::Task,
            },
        ),
        StatementKind::If(if_kind) => inside_if(if_kind, offset, schemas, scope),
        _ => scope,
    }
}

fn inside_if<'a>(
    if_kind: &IfKind,
    offset: u32,
    schemas: &'a [PluginSchema],
    scope: Scope<'a>,
) -> Scope<'a> {
    if contains(if_kind.then_branch.span, offset) {
        return walk(&if_kind.then_branch.statements, offset, schemas, scope);
    }
    match &if_kind.else_branch {
        Some(ElseBranch::Block(block)) if contains(block.span, offset) => {
            walk(&block.statements, offset, schemas, scope)
        }
        Some(ElseBranch::If(inner)) if contains(inner.span, offset) => {
            inside_if(&inner.kind, offset, schemas, scope)
        }
        _ => scope,
    }
}

/// Computes the scope opened by a block whose header is `path`, based on
/// the current `scope`: a matching plugin object field wins over the
/// core-block classification.
///
/// The path is resolved wholesale against the schema so a `<sourceSet>.deps`
/// block is still classified as [`CoreBlock::Deps`] (via its terminal
/// segment), and a nested plugin block (a `buildTypes` opened inside
/// `android`) resolves to the matching object field.
fn descend<'a>(path: &Path, scope: Scope<'a>, schemas: &'a [PluginSchema]) -> Scope<'a> {
    let plugin = match scope {
        Scope::Plugin { field } => resolve_object_field(&field.properties, &path.segments),
        // A top-level plugin block is opened by a lone name; a dotted path
        // at top level is a core construct (e.g. `commonMain.deps`) and is
        // classified below.
        Scope::Core { .. } if path.is_single() => schemas
            .iter()
            .flat_map(|s| s.properties.iter())
            .find(|f| f.name == path.head() && is_object(f)),
        Scope::Core { .. } => None,
    };
    if let Some(field) = plugin {
        return Scope::Plugin { field };
    }
    let block = match path.segments.last().map(|s| s.text.as_str()) {
        Some("deps") => CoreBlock::Deps,
        Some("run") => CoreBlock::Run,
        _ => CoreBlock::Top,
    };
    Scope::Core { block }
}

fn is_object(field: &SchemaField) -> bool {
    field.type_name == "object" && !field.properties.is_empty()
}

/// Walks `segments` against `root` (the enclosing scope's properties),
/// returning the deepest `"object"` field with nested properties. Each
/// segment must name such a field at its level; a miss returns the deepest
/// field matched so far (or `None` if the first segment missed).
fn resolve_object_field<'a>(
    root: &'a [SchemaField],
    segments: &[Ident],
) -> Option<&'a SchemaField> {
    let mut level = root;
    let mut deepest = None;
    for seg in segments {
        let next = level.iter().find(|f| f.name == seg.text && is_object(f));
        match next {
            Some(field) => {
                deepest = Some(field);
                level = &field.properties;
            }
            None => return deepest,
        }
    }
    deepest
}

/// Finds the identifier that names the statement under `offset`: the `key`
/// of a `key value` pair, or the header of a block. Walks into nested
/// blocks. Used by hover to locate the key the cursor is on inside a
/// plugin-owned block.
pub(crate) fn key_at(statements: &[Statement], offset: u32) -> Option<&Ident> {
    for stmt in statements {
        if !contains(stmt.span, offset) {
            continue;
        }
        match &stmt.kind {
            StatementKind::Pair { key, .. } if contains(key.span, offset) => return Some(key),
            StatementKind::BlockStmt { path, block } => {
                if let Some(seg) = path.segments.last()
                    && contains(seg.span, offset)
                {
                    return Some(seg);
                }
                if let Some(found) = key_at(&block.statements, offset) {
                    return Some(found);
                }
            }
            StatementKind::ConventionDef { block, .. }
            | StatementKind::FnDef { block, .. }
            | StatementKind::TaskDef { block, .. } => {
                if let Some(found) = key_at(&block.statements, offset) {
                    return Some(found);
                }
            }
            StatementKind::If(if_kind) => {
                if let Some(found) = key_at_if(if_kind, offset) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds the key under `offset` inside an `if`, recursing through the whole
/// `else if`/`else` chain so hover reaches keys in every branch.
fn key_at_if(if_kind: &IfKind, offset: u32) -> Option<&Ident> {
    if let Some(found) = key_at(&if_kind.then_branch.statements, offset) {
        return Some(found);
    }
    match &if_kind.else_branch {
        Some(ElseBranch::Block(block)) => key_at(&block.statements, offset),
        Some(ElseBranch::If(inner)) => key_at_if(&inner.kind, offset),
        None => None,
    }
}

/// Builds the [`CompletionItem`]s for a set of schema fields — the keys
/// valid inside the block those fields belong to.
pub(crate) fn properties_completions(properties: &[SchemaField]) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for prop in properties {
        match prop.type_name.as_str() {
            "object" if !prop.properties.is_empty() => {
                items.push(field_item(
                    prop,
                    CompletionItemKind::SNIPPET,
                    block_snippet(&prop.name),
                ));
            }
            "enum" => {
                items.push(field_item(prop, CompletionItemKind::ENUM, None));
                for variant in &prop.variants {
                    items.push(enum_member_item(variant, prop));
                }
            }
            _ => items.push(field_item(prop, CompletionItemKind::FIELD, None)),
        }
    }
    items
}

/// The snippet text that opens a freshly-typed block header.
fn block_snippet(name: &str) -> Option<(String, InsertTextFormat)> {
    Some((format!("{name} {{\n\t$0\n}}"), InsertTextFormat::SNIPPET))
}

fn field_item(
    prop: &SchemaField,
    kind: CompletionItemKind,
    insert: Option<(String, InsertTextFormat)>,
) -> CompletionItem {
    let mut item = CompletionItem {
        label: prop.name.clone(),
        kind: Some(kind),
        detail: Some(type_detail(prop)),
        ..Default::default()
    };
    if let Some((text, format)) = insert {
        item.insert_text = Some(text);
        item.insert_text_format = Some(format);
    }
    if !prop.description.is_empty() {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: prop.description.clone(),
        }));
    }
    item
}

fn enum_member_item(variant: &str, enum_field: &SchemaField) -> CompletionItem {
    CompletionItem {
        label: variant.to_owned(),
        kind: Some(CompletionItemKind::ENUM_MEMBER),
        detail: Some(enum_field.name.clone()),
        documentation: (!enum_field.description.is_empty()).then(|| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: enum_field.description.clone(),
            })
        }),
        ..Default::default()
    }
}

/// A human-readable type label for a schema field's `detail`.
fn type_detail(prop: &SchemaField) -> String {
    match prop.type_name.as_str() {
        "object" => "block".to_owned(),
        "array" => match &prop.items {
            Some(items) => format!("array of {items}"),
            None => "array".to_owned(),
        },
        other => other.to_owned(),
    }
}

fn contains(span: Span, offset: u32) -> bool {
    span.start <= offset && offset < span.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{CompletionItemKind, InsertTextFormat};
    use ulb_lang::parser::parse;
    use ulb_schema::SchemaField;

    fn field(name: &str, type_name: &str, description: &str) -> SchemaField {
        SchemaField {
            name: name.to_owned(),
            type_name: type_name.to_owned(),
            description: description.to_owned(),
            required: false,
            properties: vec![],
            items: None,
            variants: vec![],
        }
    }

    fn object_field(name: &str, properties: Vec<SchemaField>) -> SchemaField {
        SchemaField {
            name: name.to_owned(),
            type_name: "object".to_owned(),
            description: String::new(),
            required: false,
            properties,
            items: None,
            variants: vec![],
        }
    }

    /// The `ulite/android` plugin schema: a top-level `android` block whose
    /// keys mirror GRAMMAR.md Appendix A (a scalar, a nested build-types
    /// block, and an enum).
    fn android_schema() -> PluginSchema {
        let mut build_types =
            object_field("buildTypes", vec![field("minifyEnabled", "boolean", "")]);
        build_types.description = "Build type configuration.".to_owned();
        let mut signing = field("signingConfig", "enum", "The signing config.");
        signing.variants = vec!["debug".to_owned(), "release".to_owned()];
        let android = object_field(
            "android",
            vec![
                field("compileSdk", "integer", "The compile SDK version."),
                build_types,
                signing,
            ],
        );
        PluginSchema {
            name: "ulite/android".to_owned(),
            properties: vec![android],
        }
    }

    fn scope_at<'a>(text: &str, needle: &str, schemas: &'a [PluginSchema]) -> Scope<'a> {
        let offset = text.find(needle).expect("needle in text") as u32;
        let parsed = parse(text);
        innermost_scope(&parsed.file.statements, offset, schemas)
    }

    #[test]
    fn inside_plugin_block_yields_plugin_scope() {
        let schemas = vec![android_schema()];
        let text = "android {\n  compileSdk 37\n}\n";
        let scope = scope_at(text, "compileSdk", &schemas);
        match scope {
            Scope::Plugin { field } => assert_eq!(field.name, "android"),
            other => panic!("expected plugin scope, got {other:?}"),
        }
    }

    #[test]
    fn nested_object_scope_tracks_the_inner_field() {
        let schemas = vec![android_schema()];
        let text = "android {\n  buildTypes {\n    minifyEnabled true\n  }\n}\n";
        let scope = scope_at(text, "minifyEnabled", &schemas);
        match scope {
            Scope::Plugin { field } => assert_eq!(field.name, "buildTypes"),
            other => panic!("expected nested plugin scope, got {other:?}"),
        }
    }

    #[test]
    fn deps_block_is_core_deps_scope() {
        let schemas = vec![android_schema()];
        let text = "deps {\n  implementation coreX\n}\n";
        let scope = scope_at(text, "implementation", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Deps
            }
        );
    }

    #[test]
    fn run_block_is_core_run_scope() {
        let schemas = vec![];
        let text = "task \"a\" {\n  run {\n    exec(command=\"echo\")\n  }\n}\n";
        let scope = scope_at(text, "exec", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Run
            }
        );
    }

    #[test]
    fn task_body_is_core_task_scope() {
        let schemas = vec![];
        let text = "task \"a\" {\n  description \"x\"\n}\n";
        let scope = scope_at(text, "description", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Task
            }
        );
    }

    #[test]
    fn top_level_scope_is_core_top() {
        let schemas = vec![android_schema()];
        let text = "plugin \"android\"\napply \"s\"\n";
        let scope = scope_at(text, "apply", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Top
            }
        );
    }

    #[test]
    fn unknown_block_is_core_top_not_plugin() {
        // `deps` is not a field of the android schema, so it must not be
        // treated as a plugin-owned block.
        let schemas = vec![android_schema()];
        let text = "deps {\n  api \"g:a\"\n}\n";
        let scope = scope_at(text, "api", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Deps
            }
        );
    }

    #[test]
    fn properties_completions_surface_scalar_object_and_enum() {
        let android = object_field(
            "android",
            vec![
                field("compileSdk", "integer", "The compile SDK version."),
                object_field("buildTypes", vec![field("minifyEnabled", "boolean", "")]),
                {
                    let mut e = field("signingConfig", "enum", "The signing config.");
                    e.variants = vec!["debug".to_owned(), "release".to_owned()];
                    e
                },
            ],
        );
        let items = properties_completions(&android.properties);

        let compile = items.iter().find(|i| i.label == "compileSdk").unwrap();
        assert_eq!(compile.kind, Some(CompletionItemKind::FIELD));
        assert_eq!(compile.detail.as_deref(), Some("integer"));

        let build_types = items.iter().find(|i| i.label == "buildTypes").unwrap();
        assert_eq!(build_types.kind, Some(CompletionItemKind::SNIPPET));
        assert_eq!(
            build_types.insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );

        let signing = items.iter().find(|i| i.label == "signingConfig").unwrap();
        assert_eq!(signing.kind, Some(CompletionItemKind::ENUM));
        for variant in ["debug", "release"] {
            assert!(
                items
                    .iter()
                    .any(|i| i.label == variant && i.kind == Some(CompletionItemKind::ENUM_MEMBER)),
                "enum member {variant} missing"
            );
        }
    }

    #[test]
    fn dotted_deps_block_is_core_deps_scope() {
        // A source-set deps block (`commonMain.deps { }`) is a real
        // construct and must complete with dependency scopes, not top-level
        // keywords.
        let schemas = vec![android_schema()];
        let text = "commonMain.deps {\n  implementation coreX\n}\n";
        let scope = scope_at(text, "implementation", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Deps
            }
        );
    }

    #[test]
    fn dotted_run_block_is_core_run_scope() {
        let schemas = vec![];
        let text = "task \"a\" {\n  commonMain.run {\n    exec(command=\"echo\")\n  }\n}\n";
        let scope = scope_at(text, "exec", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Run
            }
        );
    }

    #[test]
    fn if_branch_inherits_enclosing_core_scope() {
        let schemas = vec![android_schema()];
        let text = "deps {\n  if debugBuild {\n    implementation coreX\n  }\n}\n";
        let scope = scope_at(text, "implementation", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Deps
            }
        );
    }

    #[test]
    fn if_branch_inherits_enclosing_plugin_scope() {
        let schemas = vec![android_schema()];
        let text = "android {\n  if release {\n    compileSdk 37\n  }\n}\n";
        let scope = scope_at(text, "compileSdk", &schemas);
        match scope {
            Scope::Plugin { field } => assert_eq!(field.name, "android"),
            other => panic!("expected nested plugin scope, got {other:?}"),
        }
    }

    #[test]
    fn else_if_chain_keeps_enclosing_scope() {
        let schemas = vec![];
        let text = "deps {\n  if a { implementation x }\n  else if b { api y }\n  else { compileOnly z }\n}\n";
        let scope = scope_at(text, "compileOnly", &schemas);
        assert_eq!(
            scope,
            Scope::Core {
                block: CoreBlock::Deps
            }
        );
    }

    #[test]
    fn empty_document_and_eof_resolve_to_top() {
        let schemas = vec![android_schema()];
        assert_eq!(
            innermost_scope(&[], 0, &schemas),
            Scope::Core {
                block: CoreBlock::Top
            }
        );
        // A cursor past every statement's end still resolves to the outer
        // scope rather than an interior block.
        let text = "android {\n  compileSdk 37\n}\n";
        let parsed = parse(text);
        let past_end = parsed.file.span.end;
        assert_eq!(
            innermost_scope(&parsed.file.statements, past_end, &schemas),
            Scope::Core {
                block: CoreBlock::Top
            }
        );
    }

    #[test]
    fn properties_completions_on_empty_input_is_empty() {
        assert!(properties_completions(&[]).is_empty());
    }

    #[test]
    fn object_with_empty_properties_is_a_field_not_a_snippet() {
        // An object whose properties are not declared (a dynamic map) is
        // offered as a plain field, not a `name { ... }` snippet.
        let empty = object_field("extra", vec![]);
        let items = properties_completions(&[empty]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Some(CompletionItemKind::FIELD));
        assert_eq!(items[0].insert_text, None);
    }

    #[test]
    fn enum_with_empty_variants_is_enum_only() {
        let mut e = field("variant", "enum", "");
        e.variants = vec![];
        let items = properties_completions(&[e]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Some(CompletionItemKind::ENUM));
        assert!(
            !items
                .iter()
                .any(|i| i.kind == Some(CompletionItemKind::ENUM_MEMBER))
        );
    }

    #[test]
    fn key_at_finds_pair_key_and_block_header() {
        let text = "android {\n  compileSdk 37\n  signingConfig debug\n}\n";
        let parsed = parse(text);
        let offset = text.find("compileSdk").expect("needle") as u32;
        let key = key_at(&parsed.file.statements, offset).expect("key under pair cursor");
        assert_eq!(key.text, "compileSdk");

        let offset = text.find("signingConfig").expect("needle") as u32;
        let key = key_at(&parsed.file.statements, offset).expect("key under pair cursor");
        assert_eq!(key.text, "signingConfig");
    }

    #[test]
    fn key_at_reaches_the_else_if_else_branch() {
        // Regression: the final `else` of an `else if` chain must be reached
        // by key lookup (previously only the then-branch of the inner if).
        let text = "android {\n  if a { compileSdk 1 }\n  else if b { signingConfig debug }\n}\n";
        let parsed = parse(text);
        let offset = text.find("signingConfig").expect("needle") as u32;
        let key = key_at(&parsed.file.statements, offset).expect("key in else-if else");
        assert_eq!(key.text, "signingConfig");
    }
}
