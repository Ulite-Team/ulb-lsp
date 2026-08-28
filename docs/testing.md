# Testing

The analysis engine is tested without a running LSP server. The server
adapter has no tests of its own beyond what the library covers — it is a
thin pass-through, and the behaviors it owns (dedupe, late-start adopt,
republish on conventions edit) are pinned by the engine's contracts plus
the unit tests below.

## Where the tests live

- **Doc-tests** inside `src/lib.rs`, `src/document.rs`, `src/utf16.rs`,
  `src/navigation.rs`, `src/completion.rs`, `src/role.rs` — small,
  executable examples of the public API.
- **Unit tests** in `src/document.rs` (edit application), `src/utf16.rs`
  (conversion boundary rules), `src/diagnostics.rs` (the engine's
  diagnostic classes), `src/navigation.rs` (hover/goto resolution),
  `src/plugin_schema.rs` (scope classification + completion item kinds),
  `src/completion.rs` (vocabulary, plugin-schema loading, caching,
  hover-on-field).
- **Integration tests** in `tests/worked_example.rs` — the engine against
  realistic project sources modeled on `examples/sample-kmp` from the
  Uliab repository.

## Test doubles

Role files are served from memory by `MapLoader`, a
`HashMap<PathBuf, String>` implementing `SourceLoader`. This is the same
seam the server uses with `DiskLoader`, so tests exercise exactly the
code path the server runs.

## Diagnostic behaviors pinned by tests

- Parse errors are errors (`parse_errors_are_reported_as_errors`).
- Only the unknown convention is reported, with the correct line and
  UTF-16 character range (`known_and_unknown_apply_resolution`).
- Open `conventions.ulb` shadows disk (`open_conventions_document_shadows_disk`).
- Unknown deps alias → "unknown reference" (`unknown_alias_is_reported`).
- Libs aliases resolve cleanly (`libs_aliases_resolve_cleanly`).
- Unknown function → "unknown function" (`unknown_function_call_is_reported`).
- `convention {}` in `build.ulb` → role violation
  (`convention_definition_inside_build_is_a_role_violation`).
- `env`/`props` that cannot resolve produce no diagnostics
  (`env_and_props_are_unresolved_not_errors`).
- Applies inside blocks and both `if` branches are scanned, including a
  statically-dead branch (`applies_inside_blocks_and_conditionals_are_scanned`).
- Non-build roles never scan applies (`non_build_roles_do_not_scan_applies`).
- Unknown documents produce empty diagnostics
  (`diagnostics_for_unopened_document_is_empty`).

## Navigation behaviors pinned by tests

- Hit test finds applies in blocks, `if`/`else`, and top level, and
  ignores the `apply` keyword itself (`apply_target_is_found_inside_blocks_and_conditionals`).
- Hover shows the body in a fenced `ulb` block (`hover_shows_convention_name_and_body`).
- Hover on an undefined convention names it as undefined
  (`hover_reports_unknown_convention`).
- Goto-definition points at the convention name span in `conventions.ulb`
  (`goto_definition_points_at_convention_name_in_conventions_ulb`), is
  `None` for unknown conventions, and honors an open `conventions.ulb`
  over disk.

## Completion behaviors pinned by tests

- Inside a plugin block the schema keys complete
  (`inside_plugin_block_offers_the_schema_key`); at top level both core
  words and the applied plugin's block name complete
  (`top_level_offers_core_and_plugin_block_name`).
- `deps {}` offers only dependency scopes (never `apply`); `run {}` offers
  the task actions (`inside_deps_offers_only_dependency_scopes`,
  `inside_run_offers_task_actions`).
- Non-build roles answer no completion
  (`non_build_role_yields_no_completion`).
- Hover on a key absent from the schema is `None`, and a schema is fetched
  from disk once then served from the cache
  (`hover_none_for_key_not_in_schema`, `schema_is_cached_after_first_request`).
- A missing plugin artifact degrades to core vocabulary rather than
  failing (`missing_wasm_degrades_gracefully`).
- Scope classification: `deps`/`run`/`task body`/top level are core; an
  unknown block name falls back to top-level; nested object fields track
  the inner schema field (`plugin_schema.rs` tests).

## Position handling pinned by tests

Round-trips and the clamping rules are covered in [utf16.md](utf16.md);
the Arabic and emoji cases exist because the DSL's real userbase includes
RTL content and the file must not break on it.

## Running

```sh
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

`unsafe_code = "forbid"` is set at the crate level, so the lint is part
of the build, not a suggestion.
