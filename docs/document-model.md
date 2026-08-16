# Document model

`src/document.rs` holds the server's in-memory view of open documents.

## `Document`

```rust
pub struct Document {
    pub text: String,
    pub version: i32,
}
```

Just the full current text and the editor's change counter. `apply`
performs an incremental edit:

- `range: None` → replace the whole text (the full-sync case);
- `range: Some(r)` → translate the UTF-16 range to byte offsets and
  `replace_range` in place.

Out-of-range boundaries clamp exactly as [utf16.md](utf16.md) specifies
(column past line end clamps to line end, column inside a surrogate pair
clamps to the pair's start, position past the last line clamps to source
end). Offsets are then bounded so `replace_range` never panics.

## `DocumentStore`

A `HashMap<Url, Document>` with a small, explicit API:

- `upsert(uri, doc)` — open or replace,
- `apply_change(uri, version, range, replacement) -> bool` — edit if open;
  `false` when the document is unknown,
- `get` / `remove` / `contains`,
- `len` / `is_empty`,
- `iter` over `(uri, document)`.

The `bool` return of `apply_change` is the signal `main.rs` uses to detect
the late-start case (document not tracked): when every change returns
`false`, the server adopts the last change's full text as an `upsert`.

## Ownership boundary

The store is owned by the library (`DiagnosticEngine`). The server never
parses or edits text directly; it passes protocol parameters in and the
store answers with analysis results. This is what makes the engine
testable without a server — the `Document` doc-tests and the store unit
tests edit text and observe it directly.
