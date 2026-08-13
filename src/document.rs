//! In-memory store of open documents and their text.
//!
//! The server keeps the current text of every open `.ulb` file here so the
//! engine can re-parse on every `didChange` without touching the disk.

use std::collections::HashMap;

use lsp_types::Url;

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
