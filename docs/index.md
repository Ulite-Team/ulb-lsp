# ulb-lsp — Documentation

`ulb-lsp` is a language server for the `ulb` build DSL. It speaks the LSP
over stdio and runs diagnostics, hover, and goto-definition against the
same typed AST the evaluator uses (`Uliab/crates/ulb-lang`).

## Documents

| Document | What it covers |
|---|---|
| [architecture.md](architecture.md) | Two-layer design (analysis library + thin server), what analysis is honest about, and the editor-tooling split |
| [diagnostics.md](diagnostics.md) | The analysis engine: the passes (parse, convention check, build lint, settings lint), role files, and the dedupe contract |
| [navigation.md](navigation.md) | Hover and goto-definition for `apply "name"` → `conventions.ulb` |
| [protocol.md](protocol.md) | The server process: capabilities, sync, the publish/dedupe path, republish-on-conventions-edit |
| [document-model.md](document-model.md) | `Document` / `DocumentStore`: how open-document text and versions are kept |
| [utf16.md](utf16.md) | Byte-span ↔ UTF-16 position/range conversion, and its clamping rules |
| [using.md](using.md) | Editor configuration and the requirements |
| [testing.md](testing.md) | How the engine is tested without a running server |

## The two-layer split in one paragraph

`src/lib.rs` is the analysis engine and knows nothing about the LSP
runtime: `DiagnosticEngine::diagnostics_for`, `hover`, and
`goto_definition` are plain synchronous functions a test can call. `src/main.rs`
is the tower-lsp server — document tracking, publish calls, capability
advertising — and nothing else. Every test runs against the library; the
server adapter is thin enough that its logic is covered by unit tests
where it is not shared.

## Why the LSP uses the real AST

The alternative would be a purpose-built "editor grammar" that only
highlights and reports shallow errors. This project uses the actual
`ulb-lang` parser and the actual evaluator in lint mode, so the editor
never disagrees with a build about what a document means. The cost is
that the server carries `ulb-lang` as a dependency and reparses on every
`didChange` — the files are small and the parser is fast enough that this
is not a concern yet.
