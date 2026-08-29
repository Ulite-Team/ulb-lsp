# Protocol and server

`src/main.rs` is the tower-lsp `LanguageServer` implementation. It tracks
documents, calls the analysis library, and publishes. There is no analysis
logic here.

## Transport and lifecycle

- The server speaks LSP over **stdio** (`Server::new(stdin, stdout, …)`).
  No `--tcp` flag exists; a wrapper like `tower-lsp`'s own examples or an
  editor plugin is expected to spawn `ulb-lsp` per-session.
- `server_info` reports name `ulb-lsp` and the crate version.
- Only `*.ulb` files are analyzed (`is_ulb_file`); everything else is
  ignored by every handler.

## Capabilities

| Capability | Value |
|---|---|
| `textDocumentSync` | `INCREMENTAL`, with `openClose` and `save` |
| `hoverProvider` | `true` |
| `definitionProvider` | `true` |
| `completionProvider` | `triggerCharacters: ['.']`, `resolveProvider: false` |

Position encoding is not advertised (defaults to UTF-16, which is what the
engine's conversions target — see [utf16.md](utf16.md)).

## Completion

A `textDocument/completion` handler answers on `*.ulb` files whose role
is `Build` (`build.ulb` or similar); non-build roles return `Ok(None)`.
The engine splits the cursor position into a core scope or a
plugin-owned block (via the plugin config schema) and returns the
appropriate vocabulary. See [completion.md](completion.md).

## Document lifecycle

- `did_open` — `upsert(Document::new(text, version))`, publish, and if the
  file is `conventions.ulb` republish every open `build.ulb`.
- `did_change` — apply each incremental `content_change` (range + text)
  through `engine.apply_change`. If the document was not tracked (the
  server started after the buffer was already open), the last change —
  which carries the full current text under incremental sync semantics
  when the server missed the open — is adopted as a full `upsert`.
- `did_save` — republish (cheap; dedupe skips the wire when nothing
  changed).
- `did_close` — drop the document and forget the last-published snapshot.

The "server started late" fallback is the only special case: incremental
sync means the client may assume the server already had the document text
when it sent the first change, and a server that missed `didOpen` must not
silently hold an empty buffer.

## Publishing and dedup

`publish` runs the engine, then compares against
`last_published[uri]`; identical results are **not** re-sent over the
wire. This keeps keystroke-by-keystroke diagnostics chatter off the
socket while the document is being typed. The snapshot is cleared on
`did_close`, so reopening a file always publishes once.

## Republish-on-conventions-edit

A `conventions.ulb` edit changes the definition table every open
`build.ulb` sees. Both `did_open` and `did_change` detect the role and
call `republish_builds`, which re-runs diagnostics for every open
`build.ulb` (sorted, deterministic). `libs.ulb` is not yet handled this
way; alias edits are picked up on the next publish of the affected file.

## Handler stub-by-design

`initialize`/`initialized`/`shutdown` are minimal. There is no
workspace-folder logic and no settings; the server is single-project and
reads role files adjacent to the document being analyzed.
