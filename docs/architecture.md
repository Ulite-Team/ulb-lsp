# Architecture

## Two layers

The LSP process is split so that analysis has no LSP dependency:

- **The library** (`src/lib.rs`, package name `ulb-lsp`) turns a
  `ulb-lang` parse into protocol diagnostics and navigation results. It is
  synchronous, holds no sockets, and is driven directly by unit tests.
- **The server** (`src/main.rs`, binary `ulb-lsp`) is a tower-lsp
  `LanguageServer` that tracks open documents, feeds every `didChange`
  into the library, and publishes the results.

The boundary is `DocumentStore` in the library: the server never holds
document text; it calls `upsert` / `apply_change` / `close` and the
library does all parsing.

## Analysis honesty

The engine is deliberately conservative about what it claims:

- **Parse diagnostics** always come from a real `ulb-lang` parse of the
  current buffer text (parser never fails fast — GRAMMAR.md §11 — so
  mid-edit source is useful).
- **Semantic diagnostics** on `build.ulb` come from the evaluator run in
  **lint mode** (`evaluate_build_lint`): `env()` and `props()` lookups are
  resolved to "unresolved, not an error" because the editor cannot see the
  build environment. Reporting an env failure as a hard error in the
  editor would be a lie.
- **Unknown convention** is the one check kept out of the evaluator. It is
  a targeted AST walk (`collect_applies`) so that `apply "name"` in *both*
  branches of an `if` is checked, even when the condition is statically
  false and the evaluator never reaches one branch.

## Role model

The DSL has one grammar and four file roles (`role.rs`, per GRAMMAR.md
§10). The engine parses every open `.ulb` file identically, then applies
role-specific checks only where the filename allows them:

| File | Role | Extra diagnostics |
|---|---|---|
| `settings.ulb` | Settings | settings evaluator (unknown keys, duplicate declarations, malformed blocks) |
| `build.ulb` | Build | evaluator (lint), convention resolution |
| `conventions.ulb` | Conventions | none beyond parse |
| `libs.ulb` | Libs | none beyond parse |
| anything else | Unknown | none beyond parse |

## Cross-file resolution

`conventions.ulb` and `libs.ulb` are globally visible to every
`build.ulb` (GRAMMAR.md §6.3/§6.4), so a `build.ulb`'s analysis resolves
both adjacent files through `resolve_definitions`:

1. an **open document** wins over disk (`text_of` checks the store first);
2. otherwise a `SourceLoader` reads the file (`DiskLoader` in the server,
   a map in tests).

This is why editing `conventions.ulb` forces a republish of every open
`build.ulb`: the convention table changed, so every `apply` check and
definition lookup must rerun.

## Editor-tooling split across repositories

| Concern | Repository |
|---|---|
| Presentation (highlight, fold, indent) | `Ulite-Team/tree-sitter-ulb` |
| Semantic analysis, navigation, diagnostics | `Ulite-Team/ulb-lsp` (this repo) |
| Evaluation, build, plugin host | `Ulite-Team/Uliab` |

`ulb-lsp` depends on `Uliab/crates/ulb-lang` by path. The split exists
because tree-sitter is good at presentation and bad at evaluation; the
LSP does semantics on the same AST a build would use.

## Why a path dependency

`ulb-lang` is the single source of truth for parsing and evaluation. The
LSP does not re-implement any of it — it maps `ulb-lang` diagnostics to
the protocol. A released LSP would switch to a published `ulb-lang`
version (or a workspace member) without changing any analysis code.
