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

- Inside a plugin block the schema keys complete — and the core vocabulary
  does not leak in
  (`inside_plugin_block_offers_the_schema_key`); at top level both core
  words and the applied plugin's block name complete
  (`top_level_offers_core_and_plugin_block_name`).
- `deps {}` offers only dependency scopes (never `apply`); `run {}` offers
  the task actions (`inside_deps_offers_only_dependency_scopes`,
  `inside_run_offers_task_actions`).
- Non-build roles answer no completion
  (`non_build_role_yields_no_completion`).
- Hover on a known key returns its description with the key's range
  (`hover_on_known_key_returns_field_description`); on a key absent from
  the schema it is `None`; and a schema is fetched from disk once then
  served from the cache with a correct answer
  (`hover_none_for_key_not_in_schema`, `schema_is_cached_after_first_request`).
- The public `hover` falls back to the plugin field
  (`hover_public_api_falls_back_to_plugin_field`).
- A missing plugin artifact, an unversioned plugin value, and a
  non-coordinate plugin value all degrade to core vocabulary rather than
  failing (`missing_wasm_degrades_gracefully`,
  `unversioned_plugin_value_degrades_to_core`,
  `non_coordinate_plugin_value_degrades_to_core`).
- Scope classification: `deps`/`run`/`task body`/top level are core;
  `<sourceSet>.deps` is still core-deps; `if`/`else if` chains inherit the
  enclosing scope; an unknown block name falls back to top-level; and
  nested object fields track the inner schema field (`plugin_schema.rs`
  tests: `dotted_deps_block_is_core_deps_scope`,
  `if_branch_inherits_enclosing_core_scope`,
  `else_if_chain_keeps_enclosing_scope`, `unknown_block_is_core_top_not_plugin`,
  `nested_object_scope_tracks_the_inner_field`).
- Completion-item kinds: object blocks are snippets, an object without
  nested properties is a plain field, an enum without variants is enum-only,
  and empty inputs produce no items
  (`properties_completions_surface_scalar_object_and_enum`,
  `object_with_empty_properties_is_a_field_not_a_snippet`,
  `enum_with_empty_variants_is_enum_only`,
  `properties_completions_on_empty_input_is_empty`).
- Key lookup finds pair keys and block headers, and reaches the final
  `else` of an `else if` chain (`key_at_finds_pair_key_and_block_header`,
  `key_at_reaches_the_else_if_else_branch`).

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
