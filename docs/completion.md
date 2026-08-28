# Completion

`src/completion.rs` and `src/plugin_schema.rs` implement
`textDocument/completion` and the plugin-field half of hover. The core
tool vocabulary is hardcoded in the server; completion inside
plugin-owned blocks reads the plugin's config schema.

## Scope model

`plugin_schema.rs` classifies a cursor into one of two scope families:

- `Core { block }` — a language construct: the top level (`Top`), a
  `deps {}` block, a `run {}` block, or a `task NAME { ... }` body.
- `Plugin { field }` — inside a block owned by an applied plugin (e.g.
  `android {}`), or a nested object within one (`buildTypes {}`).

`innermost_scope` walks the statement list to the innermost range
containing the cursor offset. A `BlockStmt` whose header name matches an
applied plugin produces a plugin scope; nested object fields track their
position in the schema tree. Anything else falls back to the enclosing
core scope, so an unknown block name completes as top-level rather than
silently as a plugin.

## Core vocabulary

The engine answers from a hardcoded slice of the grammar:

- Top-level statements: `plugin`, `apply`, `deps`, `task`, `if`, `else`,
  `description`.
- `deps {}` scopes: `implementation`, `api`, `testImplementation`,
  `androidTestImplementation`, `compileOnly`, `runtimeOnly`.
- `run {}` actions: `exec`, `copy`.
- `task NAME { }` body: `description`, `dependsOn`, `run`.

## Plugin schemas

Completion inside a plugin block needs that plugin's config schema. The
server derives the schema artifact path from the already-resolved plugin
value:

1. From `libs.ulb`'s `plugins {}` declarations (parsed in lint mode by
   `resolve_definitions`), each plugin name maps to a value.
2. `PluginSpec::from_value` turns that resolved value into a
   `PluginSpec`; the spec's coordinate (vendor/name + version) yields the
   cached artifact under the plugin cache dir as
   `<cache>/<vendor>/<name>/<version>/plugin.wasm`.
3. The `.wasm` is loaded through `SourceLoader::load_bytes` and its
   embedded `ulb-config-schema` custom section is decoded into a
   `PluginSchema`.

The content-addressed path (name + version) is stable, so results are
memoized in the engine's `schema_cache` for the document's lifetime — a
schema is read from disk once per build session, not per keystroke. A
missing artifact degrades gracefully: the engine still answers with the
core vocabulary rather than erroring.

## Completion

`DiagnosticEngine::completion(uri, position)`:

- `Core { Top }` → core top-level items plus the block names of every
  applied plugin (so an applied `android` plugin surfaces `android { }`).
- `Core { Deps | Run | Task }` → the scoped core vocabulary above.
- `Plugin { field }` → the schema's properties at that field: scalar
  keys as `FIELD`, object keys as `SNIPPET` with a `name {\n\t$0\n}` body,
  enum values as `ENUM`/`ENUM_MEMBER`.

## Hover on plugin fields

`DiagnosticEngine::plugin_field_hover(uri, position)` answers when the
cursor is on a config key inside a plugin scope: it returns the field's
description from the schema as a markdown doc. `hover` falls back to it
when the position is not on an `apply "name"` target.

## Non-build roles

Completion is gated on the `Build` role — `settings.ulb`, `libs.ulb`,
and `conventions.ulb` return `Ok(None)` from `completion` (and are
excluded server-side by the handler).
