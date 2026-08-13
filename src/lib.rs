//! `ulb-lsp` — analysis engine for the ulb language server.
//!
//! The LSP process has two layers: this library, which turns a
//! [`ulb_lang`] parse into protocol [`lsp_types::Diagnostic`]s, and the
//! thin tower-lsp server in `main.rs` that feeds open documents in and
//! publishes the results. Keeping the analysis free of any LSP runtime
//! makes it unit-testable without a running server.
//!
//! Diagnostics come from two passes, both over the *same* typed AST the
//! evaluator uses (ARCHITECTURE.md §11), so what the editor reports never
//! diverges from what evaluation would do:
//!
//! 1. [`diagnostics::DiagnosticEngine::diagnostics_for`] always parses the
//!    document and maps every lexer/parser diagnostic to the protocol,
//!    which is what makes mid-edit source useful (the parser never fails
//!    fast; GRAMMAR.md §11).
//! 2. For a `build.ulb`, the engine additionally resolves the project's
//!    `conventions.ulb` and `libs.ulb` and runs the evaluator in **lint
//!    mode** ([`ulb_lang::eval::evaluate_build_lint`]): `env`/`props`
//!    lookups never touch the process or filesystem, so the editor reports
//!    name, type, arity, and role-violation diagnostics without lying
//!    about a build-time environment it cannot see.
//! 3. The unknown-convention check stays a targeted AST walk rather than
//!    relying on the evaluator, so `apply "name"` statements are checked
//!    in *both* branches of an `if`, regardless of whether the condition
//!    is statically true.

#![warn(missing_docs)]

pub mod diagnostics;
pub mod document;
pub mod role;
pub mod utf16;

pub use diagnostics::{DiagnosticEngine, DiskLoader, SourceLoader};
pub use document::{Document, DocumentStore};
pub use role::{Role, role_of};
pub use utf16::span_to_range;
