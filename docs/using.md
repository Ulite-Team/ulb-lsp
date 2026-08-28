# Using ulb-lsp

## Requirements

- Rust (edition 2024).
- The `Uliab` repository on disk at `../Uliab` relative to this repo —
  the crate depends on `ulb-lang` by path. A released build would publish
  or vendor that dependency; until then the two must be checked out
  side by side.

## Building

```sh
cargo build --release --bin ulb-lsp
cargo test
cargo clippy -- -D warnings
```

The binary is `target/release/ulb-lsp`.

## Running

`ulb-lsp` reads LSP from stdin and writes responses to stdout. Point your
editor's LSP client at the binary with no arguments:

```jsonc
// example VS Code-like configuration
{
  "language": "ulb",
  "command": ["/path/to/ulb-lsp"],
  "documentSelector": [{ "pattern": "**/*.ulb" }]
}
```

## What works

- Parse and semantic diagnostics (`source: ulb-lang`) pushed on open and
  on every incremental edit, deduplicated.
- Hover on `apply "name"` showing the convention body.
- Goto-definition from `apply "name"` to the `convention NAME` in the
  adjacent `conventions.ulb`.
- Completion inside `build.ulb`: core vocabulary per scope plus the keys
  of plugin-owned blocks (`android {}`, nested objects) read from the
  plugin's cached config-schema artifact.
- Hover on a plugin-owned block's config key shows the field description
  from the same schema.
- Re-analysis of all open `build.ulb` files when `conventions.ulb` changes.

## What does not exist yet

- Signature help, document symbols, code actions, rename.
- Any UI for configuration; the server is single-project by design.
- Multi-root workspaces.

## Reporting diagnostics correctly

Open `build.ulb` files alongside their `conventions.ulb`/`libs.ulb`. The
engine falls back to disk when they are not open, but unsaved edits to
role files only take effect while those files are open in the editor.
