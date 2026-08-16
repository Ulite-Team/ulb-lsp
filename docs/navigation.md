# Navigation

`src/navigation.rs` implements hover and goto-definition for one
navigation target: an `apply "name"` statement resolving to its
`convention NAME` definition in the adjacent `conventions.ulb`.

Both are methods on `DiagnosticEngine` (`hover`, `goto_definition`) so
they share the open-document store and `SourceLoader` with diagnostics —
an open `conventions.ulb` shadows the disk, exactly like diagnostics.

## The resolution pipeline

For a cursor position in a `build.ulb`:

1. `apply_target_at` converts the LSP position to a byte offset
   (`utf16.rs`) and finds the `apply` statement whose **name string** —
   not the `apply` keyword — contains that offset.
2. `convention_locations` parses `conventions.ulb` and maps every
   top-level `convention NAME { ... }` to its definition site (name span +
   body span, in that file's byte space).
3. The target name is looked up in that map.

## Scope of the hit test

`apply_at_offset` walks nested blocks, `task {}`, `convention {}`,
`fn` bodies, and `if`/`else` chains, so an apply nested two levels deep
inside a conditional is reachable. The hit region is the quoted name only:
hovering the `apply` keyword returns nothing (`hover_outside_apply_is_none`).

## Hover

Returns markdown in a fenced `ulb` block:

```markdown
**convention `signed`**
```ulb
minifyEnabled true
```
```

The body is sliced from the byte span captured at definition time, so it
is always the live text. For an apply to an undefined convention the
content is `convention `X` is not defined` — the same message class as the
diagnostic, so hover and gutter agree.

## Goto-definition

Returns a scalar location whose range is the `convention NAME` **name
identifier** span in `conventions.ulb` — cursor lands on the name, not
the block. Undefined conventions return `None`, and the server answers
`Ok(None)`.

## Why only conventions

The same machinery extends to other cross-file targets (a `libs.ulb`
alias in `deps {}`, a `fn` call site) without changing the structure:
find target under cursor → parse the owning file → locate definition.
Only conventions are wired up today; the targets map is `BTreeMap` so
definition lookup is deterministic.
