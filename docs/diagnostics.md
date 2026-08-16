# Diagnostics

`src/diagnostics.rs` is the analysis engine. Everything here is
synchronous and LSP-runtime-free; the server calls it and publishes what
it returns.

## Entry point

`DiagnosticEngine::diagnostics_for(uri)` produces the diagnostic list for
one open document, in three passes:

1. `parse_diagnostics` — every lexer/parser diagnostic from `ulb-lang`,
   mapped with its real span and severity (`source: "ulb-lang"`).
2. For a `build.ulb` only: `unknown_convention_diagnostics` — an AST walk
   reporting `apply "name"` where `name` is not defined.
3. For a `build.ulb` only: `evaluation_diagnostics` — the evaluator run in
   lint mode, its diagnostics mapped the same way.

A document that is not open returns an empty list; the server never asks
about anything else.

## Severity mapping

`ulb-lang` severities map 1:1 onto LSP: `Error → ERROR`,
`Warning → WARNING`, `Info → INFORMATION`. Both lexer/parser and
evaluator diagnostics carry the same `source` value (`ulb-lang`), so an
editor can group them or style them identically.

## The unknown-convention walk

The evaluator reports "unknown convention" too, but the engine drops
those reports and owns the check in a separate AST walk
(`collect_applies`). The walk recurses into block statements, `if`/`else`
branches (including `else if` chains), `task {}`, `convention {}`, and
`fn` bodies. The difference from the evaluator's behavior:

- the evaluator only visits a branch it executes;
- the walk checks both branches regardless of whether the condition is
  statically true.

Result: `apply "ghost"` inside a dead `else` is still reported. The test
`applies_inside_blocks_and_conditionals_are_scanned` pins this exact
behavior, including the case where the walk reports the unknown reference
in the `if` condition and the applies in both branches at once.

## Role files

Definitions a `build.ulb` sees come from the adjacent `conventions.ulb`
and `libs.ulb`, resolved through `resolve_definitions`:

- each file is read from the open-document store if open, else through
  the `SourceLoader` (`DiskLoader` in the server);
- each is parsed and its conventions / aliases / functions collected in
  lint mode, so `env`/`props` inside them never hit the process or disk;
- the `Definitions` set is what the evaluator lint pass and the
  convention-name check both consume.

Diagnostics *about* the role files themselves are dropped here; they
surface when that file is analyzed in its own right.

## Source loader

```rust
pub trait SourceLoader {
    fn load(&self, path: &Path) -> Option<String>;
}
```

`DiskLoader` reads the filesystem; tests use `MapLoader` to serve role
files from memory. The loader exists so analysis works on a fresh checkout
where `conventions.ulb` is not open in the editor — but an open document
always shadows the loader, so unsaved edits are honored.

## Semantics of the reported classes

The engine deliberately produces these outcomes, all pinned by tests:

- `env("X")` / `props(path).key` where the value cannot be resolved →
  **no error** (`env_and_props_are_unresolved_not_errors`). The editor
  cannot know the build environment.
- Unknown alias in `deps {}` → "unknown reference" error
  (`unknown_alias_is_reported`).
- Unknown function call → "unknown function" error
  (`unknown_function_call_is_reported`).
- `convention NAME {}` inside `build.ulb` → role violation error
  (`convention_definition_inside_build_is_a_role_violation`).
- Non-build files are never scanned for applies
  (`non_build_roles_do_not_scan_applies`).

## Deduplication contract with the server

The server publishes only when the diagnostics actually changed (see
[protocol.md](protocol.md)). That comparison is a plain `Vec` equality on
the `lsp_types::Diagnostic` values, so the engine must be deterministic:
open documents are iterated sorted by URI, diagnostics are collected in
source order, and nothing depends on hash-map ordering.
