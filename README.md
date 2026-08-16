# ulb-lsp

A language server for the `ulb` build DSL, built on the same parser and
AST the evaluator uses (`Uliab/crates/ulb-lang`).

The server is deliberately two layers. The library (`ulb_lsp`) turns a
`ulb-lang` parse into LSP diagnostics, hover, and goto-definition — plain
synchronous functions with no LSP runtime, so every behavior is
unit-testable without a running server. The binary (`ulb-lsp`) is a thin
tower-lsp adapter that tracks open documents and publishes what the
library returns.

## What it does

- Parse and semantic diagnostics pushed on open and on every incremental
  edit (deduplicated over the wire).
- Semantic analysis through the evaluator in lint mode: unknown
  references, arity/type errors, role violations — with `env()`/`props()`
  honestly reported as *unresolved* rather than as errors the editor
  cannot know about.
- An `apply "name"` convention check that walks both branches of an `if`,
  so dead code is still checked.
- Hover and goto-definition from `apply "name"` to the `convention NAME`
  in the adjacent `conventions.ulb`.
- Re-analysis of all open `build.ulb` files when `conventions.ulb`
  changes.
- UTF-16-correct positions for incremental edits, including multibyte
  (RTL) text and surrogate pairs.

## Requirements

`ulb-lang` is a path dependency, so this repo expects a sibling checkout:

```
somewhere/
  Uliab/          # provides crates/ulb-lang
  ulb-lsp/        # this repo
```

## Build and test

```sh
cargo build --release
cargo test
cargo clippy -- -D warnings
```

## Editors

Point an LSP client at `target/release/ulb-lsp` with a `**/*.ulb`
document selector. The server speaks stdio LSP with incremental sync,
hover, and definition support. See [docs/using.md](docs/using.md).

## Documentation

The full documentation lives in [`docs/`](docs/index.md):
[architecture](docs/architecture.md), [diagnostics](docs/diagnostics.md),
[navigation](docs/navigation.md), [protocol](docs/protocol.md),
[document model](docs/document-model.md), [UTF-16 handling](docs/utf16.md),
and [testing](docs/testing.md).

## License

GPL-3.0. See `LICENSE`.
