//! The `ulb-lsp` server process.
//!
//! This is the thin tower-lsp adapter over the `ulb_lsp` library: it tracks
//! open documents, runs the analysis engine on every `didChange`, and
//! publishes the resulting diagnostics. All analysis lives in the library.

use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    InitializeParams, InitializeResult, InitializedParams, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use ulb_lsp::diagnostics::{DiagnosticEngine, DiskLoader};
use ulb_lsp::document::Document;
use ulb_lsp::role::{Role, role_of};

/// The server backend: the tower-lsp client handle plus the shared analysis
/// engine.
struct Backend {
    client: Client,
    engine: Mutex<DiagnosticEngine<DiskLoader>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            engine: Mutex::new(DiagnosticEngine::new()),
        }
    }

    fn is_ulb_file(uri: &Url) -> bool {
        uri.path().ends_with(".ulb")
    }

    /// Publishes diagnostics for one document.
    async fn publish(&self, uri: Url) {
        let diagnostics = self
            .engine
            .lock()
            .expect("engine lock")
            .diagnostics_for(&uri);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    /// Re-runs diagnostics for every open `build.ulb`. Needed whenever the
    /// convention table changes (a `conventions.ulb` edit), because those
    /// documents' `apply` checks depend on it.
    async fn republish_builds(&self) {
        let build_uris = self.engine.lock().expect("engine lock").build_uris();
        for uri in build_uris {
            self.publish(uri).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "ulb-lsp".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        if !Self::is_ulb_file(&uri) {
            return;
        }
        self.engine.lock().expect("engine lock").upsert(
            uri.clone(),
            Document::new(params.text_document.text, params.text_document.version),
        );
        self.publish(uri.clone()).await;
        if role_of(&uri) == Role::Conventions {
            self.republish_builds().await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if !Self::is_ulb_file(&uri) {
            return;
        }
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        self.engine.lock().expect("engine lock").upsert(
            uri.clone(),
            Document::new(change.text, params.text_document.version),
        );
        self.publish(uri.clone()).await;
        if role_of(&uri) == Role::Conventions {
            self.republish_builds().await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if !Self::is_ulb_file(&uri) {
            return;
        }
        self.publish(uri).await;
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::build(Backend::new).finish();
    Server::new(stdin, stdout, socket).serve(service).await;
}
