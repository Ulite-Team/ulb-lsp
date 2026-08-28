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
use ulb_lang::ast::{ElseBranch, Ident, IfKind, Statement, StatementKind};
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
            let child = descend(path.head(), scope, schemas);
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

/// Computes the scope opened by a block whose header is `head`, based on
/// the current `scope`: a matching plugin object field wins over the
/// core-block classification.
fn descend<'a>(head: &str, scope: Scope<'a>, schemas: &'a [PluginSchema]) -> Scope<'a> {
    let matches =
        |f: &&SchemaField| f.name == head && f.type_name == "object" && !f.properties.is_empty();
    let found = match scope {
        Scope::Plugin { field } => field.properties.iter().find(matches),
        Scope::Core { .. } => schemas
            .iter()
            .flat_map(|s| s.properties.iter())
            .find(matches),
    };
    if let Some(field) = found {
        return Scope::Plugin { field };
    }
    let block = match head {
        "deps" => CoreBlock::Deps,
        "run" => CoreBlock::Run,
        _ => CoreBlock::Top,
    };
    Scope::Core { block }
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
                if let Some(found) = key_at(&if_kind.then_branch.statements, offset) {
                    return Some(found);
                }
                match &if_kind.else_branch {
                    Some(ElseBranch::Block(block)) => {
                        if let Some(found) = key_at(&block.statements, offset) {
                            return Some(found);
                        }
                    }
                    Some(ElseBranch::If(inner)) => {
                        if let Some(found) = key_at(&inner.kind.then_branch.statements, offset) {
                            return Some(found);
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }
    None
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
        assert!(
            items
                .iter()
                .any(|i| i.label == "release" && i.kind == Some(CompletionItemKind::ENUM_MEMBER))
        );
    }
}
