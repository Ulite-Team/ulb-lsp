//! The `ulb-lsp` server process.
//!
//! This is the thin tower-lsp adapter over the `ulb_lsp` library: it tracks
//! open documents, runs the analysis engine on every `didChange`, and
//! publishes the resulting diagnostics. All analysis lives in the library.

use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, OneOf,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use ulb_lsp::diagnostics::{DiagnosticEngine, DiskLoader};
use ulb_lsp::document::Document;
use ulb_lsp::role::{Role, role_of};

/// The server backend: the tower-lsp client handle plus the shared analysis
/// engine and the last diagnostics published per document (so unchanged
/// results are not resent).
struct Backend {
    client: Client,
    engine: Mutex<DiagnosticEngine<DiskLoader>>,
    last_published: Mutex<HashMap<Url, Vec<Diagnostic>>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            engine: Mutex::new(DiagnosticEngine::new()),
            last_published: Mutex::new(HashMap::new()),
        }
    }

    fn is_ulb_file(uri: &Url) -> bool {
        uri.path().ends_with(".ulb")
    }

    /// Publishes diagnostics for one document unless they are unchanged
    /// since the last publish.
    async fn publish(&self, uri: Url) {
        let diagnostics = self
            .engine
            .lock()
            .expect("engine lock")
            .diagnostics_for(&uri);
        {
            let mut published = self.last_published.lock().expect("published lock");
            if published.get(&uri) == Some(&diagnostics) {
                return;
            }
            published.insert(uri.clone(), diagnostics.clone());
        }
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
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
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
        let version = params.text_document.version;
        {
            let mut engine = self.engine.lock().expect("engine lock");
            let mut applied = false;
            for change in &params.content_changes {
                applied |= engine.apply_change(&uri, version, change.range, change.text.clone());
            }
            if !applied {
                // The document is not tracked (e.g. the server started
                // after the buffer was already open): the last change
                // carries the full current text, so adopt it as-is.
                if let Some(change) = params.content_changes.into_iter().last() {
                    engine.upsert(uri.clone(), Document::new(change.text, version));
                }
            }
        }
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

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.engine.lock().expect("engine lock").close(&uri);
        self.last_published
            .lock()
            .expect("published lock")
            .remove(&uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let position = params.text_document_position_params;
        if !Self::is_ulb_file(&position.text_document.uri) {
            return Ok(None);
        }
        Ok(self
            .engine
            .lock()
            .expect("engine lock")
            .hover(&position.text_document.uri, position.position))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        if !Self::is_ulb_file(&position.text_document.uri) {
            return Ok(None);
        }
        Ok(self
            .engine
            .lock()
            .expect("engine lock")
            .goto_definition(&position.text_document.uri, position.position))
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::build(Backend::new).finish();
    Server::new(stdin, stdout, socket).serve(service).await;
}
