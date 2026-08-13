//! In-memory store of open documents and their text.
//!
//! The server keeps the current text of every open `.ulb` file here so the
//! engine can re-parse on every `didChange` without touching the disk.

use std::collections::HashMap;

use lsp_types::{Range, Url};

use crate::utf16::range_to_offsets;

/// The current text and version of one open document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// The full source text as the editor last reported it.
    pub text: String,
    /// The editor's change counter for this document.
    pub version: i32,
}

impl Document {
    /// Creates a document from its text and version.
    #[must_use]
    pub fn new(text: String, version: i32) -> Self {
        Self { text, version }
    }

    /// Applies an incremental change to this document's text and records
    /// `version`. A `range` of `None` replaces the whole text (the
    /// full-sync case); a concrete `range` replaces the UTF-16 region it
    /// addresses, clamping out-of-range boundaries like
    /// [`crate::utf16::position_to_offset`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lsp_types::{Position, Range};
    /// use ulb_lsp::document::Document;
    ///
    /// let mut doc = Document::new("android { version 1 }\n".to_owned(), 1);
    /// doc.apply(
    ///     2,
    ///     Some(Range::new(Position::new(0, 18), Position::new(0, 19))),
    ///     "2".to_owned(),
    /// );
    /// assert_eq!(doc.text, "android { version 2 }\n");
    /// ```
    pub fn apply(&mut self, version: i32, range: Option<Range>, replacement: String) {
        self.version = version;
        let Some(range) = range else {
            self.text = replacement;
            return;
        };
        let (start, end) = range_to_offsets(&self.text, range);
        let start = usize::try_from(start).expect("offset fits usize");
        let end = usize::try_from(end).expect("offset fits usize");
        self.text.replace_range(start..end, &replacement);
    }
}

/// A map from document URI to its [`Document`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentStore {
    docs: HashMap<Url, Document>,
}

impl DocumentStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces the document at `uri`.
    pub fn upsert(&mut self, uri: Url, document: Document) {
        self.docs.insert(uri, document);
    }

    /// Applies an incremental change to the document at `uri`, if it is
    /// open, updating its version. Returns whether a document was present
    /// and updated.
    pub fn apply_change(
        &mut self,
        uri: &Url,
        version: i32,
        range: Option<Range>,
        replacement: String,
    ) -> bool {
        let Some(document) = self.docs.get_mut(uri) else {
            return false;
        };
        document.apply(version, range, replacement);
        true
    }

    /// Returns the document at `uri`, if any.
    #[must_use]
    pub fn get(&self, uri: &Url) -> Option<&Document> {
        self.docs.get(uri)
    }

    /// Removes the document at `uri`, returning it if present.
    pub fn remove(&mut self, uri: &Url) -> Option<Document> {
        self.docs.remove(uri)
    }

    /// Whether `uri` is currently open.
    #[must_use]
    pub fn contains(&self, uri: &Url) -> bool {
        self.docs.contains_key(uri)
    }

    /// Number of open documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether no documents are open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Iterates over the open `(uri, document)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&Url, &Document)> {
        self.docs.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn url(path: &str) -> Url {
        Url::from_file_path(path).expect("absolute file path")
    }

    #[test]
    fn apply_replaces_utf16_range_on_multiline_text() {
        let mut doc = Document::new("line one\nline two\n".to_owned(), 1);
        doc.apply(
            2,
            Some(Range::new(Position::new(0, 5), Position::new(0, 8))),
            "1".to_owned(),
        );
        assert_eq!(doc.text, "line 1\nline two\n");
        assert_eq!(doc.version, 2);
    }

    #[test]
    fn apply_multibyte_range_uses_utf16_boundaries() {
        let mut doc = Document::new("تصيير x 1\n".to_owned(), 1);
        doc.apply(
            2,
            Some(Range::new(Position::new(0, 6), Position::new(0, 7))),
            "y".to_owned(),
        );
        assert_eq!(doc.text, "تصيير y 1\n");
    }

    #[test]
    fn apply_none_range_replaces_whole_text() {
        let mut doc = Document::new("old text".to_owned(), 1);
        doc.apply(2, None, "brand new".to_owned());
        assert_eq!(doc.text, "brand new");
        assert_eq!(doc.version, 2);
    }

    #[test]
    fn store_apply_change_updates_open_document() {
        let mut store = DocumentStore::new();
        let uri = url("/proj/build.ulb");
        store.upsert(uri.clone(), Document::new("apply \"a\"\n".to_owned(), 1));
        let applied = store.apply_change(
            &uri,
            2,
            Some(Range::new(Position::new(0, 7), Position::new(0, 8))),
            "b".to_owned(),
        );
        assert!(applied);
        assert_eq!(store.get(&uri).expect("open").text, "apply \"b\"\n");
        assert_eq!(store.get(&uri).expect("open").version, 2);
    }

    #[test]
    fn store_apply_change_missing_uri_returns_false() {
        let mut store = DocumentStore::new();
        let uri = url("/proj/build.ulb");
        let applied = store.apply_change(&uri, 1, None, "x".to_owned());
        assert!(!applied);
        assert!(!store.contains(&uri));
    }
}
