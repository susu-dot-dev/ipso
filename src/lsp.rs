use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use lru::LruCache;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::diagnostics::{
    compute_own_diagnostics, compute_state_diagnostics, Diagnostic, Severity,
};
use crate::json_path::{jpath, json_path_range, LineIndex};
use crate::notebook::CellExt;
use crate::shas::compute_cell_sha;

struct LspBackend {
    client: Client,
    // Keyed by cell SHA. Only own diagnostics (DiffConflict, InvalidField) are
    // cached; state diagnostics depend on other cells and are always recomputed.
    cell_cache: Arc<Mutex<LruCache<String, Vec<Diagnostic>>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for LspBackend {
    async fn initialize(&self, _params: InitializeParams) -> LspResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "nota-bene".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Ask the editor to notify us when .ipynb files change on disk
        // (e.g. after `nb accept` modifies the file externally).
        let registration = Registration {
            id: "nb-ipynb-watcher".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/*.ipynb".to_string()),
                        kind: Some(WatchKind::Change),
                    }],
                })
                .unwrap(),
            ),
        };
        if let Err(_e) = self.client.register_capability(vec![registration]).await {}

        self.client
            .log_message(MessageType::INFO, "nota-bene LSP ready")
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        self.analyze(uri, text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params
            .content_changes
            .first()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        self.analyze(uri, text).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text.clone().unwrap_or_default();
        self.analyze(uri, text).await;
    }

    async fn did_close(&self, _params: DidCloseTextDocumentParams) {}

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in &params.changes {
            if change.uri.path().ends_with(".ipynb") {
                if let Ok(text) = std::fs::read_to_string(change.uri.path()) {
                    self.analyze(change.uri.clone(), text).await;
                }
            }
        }
    }
}

impl LspBackend {
    async fn analyze(&self, uri: Url, text: String) {
        let cache = Arc::clone(&self.cell_cache);
        let maybe_diagnostics = tokio::task::spawn_blocking(move || {
            let mut cache = cache.lock().unwrap();
            compute_lsp_diagnostics(&text, &mut cache)
        })
        .await
        .unwrap_or(None);
        // On parse failure, keep previous diagnostics (avoid flicker).
        if let Some(all_diagnostics) = maybe_diagnostics {
            self.client
                .publish_diagnostics(uri, all_diagnostics, None)
                .await;
        }
    }
}

fn compute_lsp_diagnostics(
    text: &str,
    cell_cache: &mut LruCache<String, Vec<Diagnostic>>,
) -> Option<Vec<lsp_types::Diagnostic>> {
    let nb: nbformat::v4::Notebook = serde_json::from_str(text).ok()?;

    let line_index = LineIndex::new(text);
    let mut all_lsp: Vec<lsp_types::Diagnostic> = Vec::new();

    for (cell_index, cell) in nb.cells.iter().enumerate() {
        if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
            continue;
        }

        let sha = compute_cell_sha(cell);
        let cell_id = cell.cell_id_str();

        let own_diags = if let Some(cached) = cell_cache.get(&sha) {
            cached.clone()
        } else {
            let fresh = compute_own_diagnostics(cell);
            cell_cache.put(sha, fresh.clone());
            fresh
        };

        let state_diags = compute_state_diagnostics(&nb, cell_index);

        for diag in state_diags.iter().chain(own_diags.iter()) {
            if let Some(lsp_diag) =
                map_to_lsp_diagnostic(text, &line_index, cell_index, cell_id, diag)
            {
                all_lsp.push(lsp_diag);
            }
        }
    }

    Some(all_lsp)
}

fn map_to_lsp_diagnostic(
    text: &str,
    line_index: &LineIndex,
    cell_index: usize,
    cell_id: &str,
    diag: &Diagnostic,
) -> Option<lsp_types::Diagnostic> {
    let source_path = jpath!["cells", cell_index, "source"];
    let byte_range = json_path_range(text, &source_path)?;

    let (start_line, start_char) = line_index.offset_to_position(byte_range.start);
    let (end_line, end_char) = line_index.offset_to_position(byte_range.end);
    let range = Range {
        start: Position {
            line: start_line as u32,
            character: start_char as u32,
        },
        end: Position {
            line: end_line as u32,
            character: end_char as u32,
        },
    };

    Some(lsp_types::Diagnostic {
        range,
        severity: Some(match diag.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diag.r#type.to_string())),
        source: Some("nota-bene".to_string()),
        message: format!(
            "[{}] {}: {}",
            cell_index,
            &cell_id[..cell_id.len().min(8)],
            diag.message
        ),
        ..Default::default()
    })
}

pub async fn run_server() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| LspBackend {
        client,
        cell_cache: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap()))),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::{blank_cell_metadata, CellExt};
    use crate::shas::compute_cell_sha;
    use lru::LruCache;
    use nbformat::v4::{Cell, CellId, Metadata, Notebook};
    use serde_json::json;
    use std::num::NonZeroUsize;

    fn fresh_cache() -> LruCache<String, Vec<Diagnostic>> {
        LruCache::new(NonZeroUsize::new(256).unwrap())
    }

    fn cid(s: &str) -> CellId {
        CellId::new(s).unwrap()
    }

    fn sha_entry(cell: &Cell) -> serde_json::Value {
        json!({ "cell_id": cell.cell_id_str(), "sha": compute_cell_sha(cell) })
    }

    fn nb_text(nb: &Notebook) -> String {
        serde_json::to_string_pretty(nb).unwrap()
    }

    fn plain_code_cell(id: &str, source: &str) -> Cell {
        Cell::Code {
            id: cid(id),
            metadata: blank_cell_metadata(),
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_shas(id: &str, source: &str, shas: serde_json::Value) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional
            .insert("nota-bene".to_string(), json!({ "shas": shas }));
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn cell_with_diff_and_shas(
        id: &str,
        source: &str,
        diff: &str,
        shas: serde_json::Value,
    ) -> Cell {
        let mut meta = blank_cell_metadata();
        meta.additional.insert(
            "nota-bene".to_string(),
            json!({ "shas": shas, "diff": diff }),
        );
        Cell::Code {
            id: cid(id),
            metadata: meta,
            execution_count: None,
            source: vec![source.to_string()],
            outputs: vec![],
        }
    }

    fn notebook(cells: Vec<Cell>) -> Notebook {
        Notebook {
            metadata: Metadata::default(),
            nbformat: 4,
            nbformat_minor: 5,
            cells,
        }
    }

    #[test]
    fn parse_failure_returns_none() {
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics("not json at all", &mut cache);
        assert!(diags.is_none());
    }

    #[test]
    fn valid_notebook_no_diagnostics() {
        let c1_plain = plain_code_cell("c1", "x = 1");
        let shas = json!([sha_entry(&c1_plain)]);
        let c1 = cell_with_shas("c1", "x = 1", shas);
        let nb = notebook(vec![c1]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert!(diags.is_empty(), "unexpected: {:?}", diags);
    }

    #[test]
    fn missing_cell_produces_error() {
        let nb = notebook(vec![plain_code_cell("c1", "x = 1")]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            diags[0].code,
            Some(NumberOrString::String("missing".to_string()))
        );
    }

    #[test]
    fn needs_review_produces_warning() {
        let c1_old = plain_code_cell("c1", "x = 1");
        let shas = json!([sha_entry(&c1_old)]);
        let c1 = cell_with_shas("c1", "x = 999", shas);
        let nb = notebook(vec![c1]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert!(
            diags.iter().any(|d| d.code
                == Some(NumberOrString::String("needs_review".to_string()))
                && d.severity == Some(DiagnosticSeverity::WARNING)),
            "got: {:?}",
            diags
        );
    }

    #[test]
    fn ancestor_modified_produces_warning() {
        let c1_old = plain_code_cell("c1", "x = 1");
        let c2_plain = plain_code_cell("c2", "y = 2");
        let shas = json!([sha_entry(&c1_old), sha_entry(&c2_plain)]);
        let c1 = plain_code_cell("c1", "x = 999");
        let c2 = cell_with_shas("c2", "y = 2", shas);
        let nb = notebook(vec![c1, c2]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert!(
            diags
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("ancestor_modified".to_string()))),
            "got: {:?}",
            diags
        );
    }

    #[test]
    fn diff_conflict_produces_error() {
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let c1_plain = plain_code_cell("c1", "completely different");
        let shas = json!([sha_entry(&c1_plain)]);
        let c1 = cell_with_diff_and_shas("c1", "completely different", &bad_diff, shas);
        let nb = notebook(vec![c1]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert!(
            diags.iter().any(|d| d.code
                == Some(NumberOrString::String("diff_conflict".to_string()))
                && d.severity == Some(DiagnosticSeverity::ERROR)),
            "got: {:?}",
            diags
        );
    }

    #[test]
    fn cache_hit_reuses_own_diagnostics() {
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let c1_plain = plain_code_cell("c1", "completely different");
        let shas = json!([sha_entry(&c1_plain)]);
        let c1 = cell_with_diff_and_shas("c1", "completely different", &bad_diff, shas);
        let nb = notebook(vec![c1]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags1 = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        let diags2 = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert_eq!(diags1.len(), diags2.len());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_miss_on_sha_change() {
        let bad_diff = "--- a\n+++ b\n@@ -1 +1 @@\n-old line\n+new line\n".to_string();
        let c1_plain = plain_code_cell("c1", "version A");
        let shas_a = json!([sha_entry(&c1_plain)]);
        let c1_a = cell_with_diff_and_shas("c1", "version A", &bad_diff, shas_a);
        let nb_a = notebook(vec![c1_a]);

        let c1_plain_b = plain_code_cell("c1", "version B");
        let shas_b = json!([sha_entry(&c1_plain_b)]);
        let c1_b = cell_with_diff_and_shas("c1", "version B", &bad_diff, shas_b);
        let nb_b = notebook(vec![c1_b]);

        let mut cache = fresh_cache();
        compute_lsp_diagnostics(&nb_text(&nb_a), &mut cache);
        assert_eq!(cache.len(), 1);
        compute_lsp_diagnostics(&nb_text(&nb_b), &mut cache);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn state_diagnostics_always_fresh() {
        // c2's SHA is stable; only its ancestor changes between calls.
        let c1_v1 = plain_code_cell("c1", "x = 1");
        let c2_plain = plain_code_cell("c2", "y = 2");
        let shas = json!([sha_entry(&c1_v1), sha_entry(&c2_plain)]);
        let c2 = cell_with_shas("c2", "y = 2", shas.clone());

        let nb1 = notebook(vec![plain_code_cell("c1", "x = 1"), c2.clone()]);
        let mut cache = fresh_cache();
        let diags1 = compute_lsp_diagnostics(&nb_text(&nb1), &mut cache).unwrap();
        assert!(
            !diags1
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("ancestor_modified".to_string()))),
            "unexpected ancestor_modified in first call"
        );

        // c2 SHA hits cache, but state diagnostics must still catch the ancestor change.
        let nb2 = notebook(vec![plain_code_cell("c1", "x = 999"), c2.clone()]);
        let diags2 = compute_lsp_diagnostics(&nb_text(&nb2), &mut cache).unwrap();
        assert!(
            diags2
                .iter()
                .any(|d| d.code == Some(NumberOrString::String("ancestor_modified".to_string()))),
            "expected ancestor_modified in second call"
        );
    }

    #[test]
    fn diagnostic_range_points_to_source() {
        let c1_old = plain_code_cell("c1", "x = 1");
        let shas = json!([sha_entry(&c1_old)]);
        let c1 = cell_with_shas("c1", "x = 999", shas);
        let nb = notebook(vec![c1]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        let d = diags
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("needs_review".to_string())))
            .unwrap();
        let expected = json_path_range(&text, &jpath!["cells", 0usize, "source"]).unwrap();
        let line_index = LineIndex::new(&text);
        let (sl, sc) = line_index.offset_to_position(expected.start);
        assert_eq!(d.range.start.line, sl as u32);
        assert_eq!(d.range.start.character, sc as u32);
    }

    #[test]
    fn missing_diagnostic_points_to_source_range() {
        let nb = notebook(vec![plain_code_cell("c1", "x = 1")]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        let d = diags
            .iter()
            .find(|d| d.code == Some(NumberOrString::String("missing".to_string())))
            .unwrap();
        let expected = json_path_range(&text, &jpath!["cells", 0usize, "source"]).unwrap();
        let line_index = LineIndex::new(&text);
        let (sl, sc) = line_index.offset_to_position(expected.start);
        assert_eq!(d.range.start.line, sl as u32);
        assert_eq!(d.range.start.character, sc as u32);
    }

    /// Integration test: verify that LSP diagnostic ranges for `simple.ipynb`
    /// point to the correct line/column positions in the actual fixture file.
    ///
    /// Expected positions are hardcoded (computed independently of
    /// `json_path_range` / `LineIndex`) so the test actually validates the
    /// production code rather than comparing it against itself.
    #[test]
    fn lsp_diagnostic_line_numbers_match_fixture() {
        let fixture_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple.ipynb");
        let text =
            std::fs::read_to_string(&fixture_path).expect("read tests/fixtures/simple.ipynb");

        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();

        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("missing".to_string())))
            .collect();
        assert_eq!(
            missing.len(),
            4,
            "expected exactly 4 'missing' diagnostics (one per code cell); got: {:?}",
            diags
        );

        // (cell_index, cell_id, start_line, start_col, end_line, end_col)
        // All values 0-indexed, derived from the raw fixture JSON.
        let cases: &[(usize, &str, u32, u32, u32, u32)] = &[
            (1, "plain-data", 18, 13, 20, 4),
            (2, "compute-total", 42, 13, 44, 4),
            (3, "reviewed-pass", 58, 13, 60, 4),
            (4, "b451fcb4-6e4d-481c-962f-d9bd647e9419", 68, 13, 68, 15),
        ];

        for &(cell_index, label, sl, sc, el, ec) in cases {
            let diag = missing
                .iter()
                .find(|d| d.message.contains(&format!("[{cell_index}]")))
                .unwrap_or_else(|| {
                    panic!("no 'missing' diagnostic for cell index {cell_index} ({label})")
                });

            assert_eq!(diag.range.start.line, sl, "{label} start line");
            assert_eq!(diag.range.start.character, sc, "{label} start col");
            assert_eq!(diag.range.end.line, el, "{label} end line");
            assert_eq!(diag.range.end.character, ec, "{label} end col");
        }
    }

    #[test]
    fn markdown_cell_skipped() {
        let md = nbformat::v4::Cell::Markdown {
            id: cid("m1"),
            metadata: blank_cell_metadata(),
            source: vec!["# Hello".to_string()],
            attachments: None,
        };
        let nb = notebook(vec![md]);
        let text = nb_text(&nb);
        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();
        assert!(diags.is_empty());
    }

    /// Regression test: every code cell in simple.ipynb should produce a
    /// diagnostic. Currently (broken) only 3 are emitted; the hello-world and
    /// b451fcb4 cells at the end are silently dropped.
    #[test]
    fn lsp_diagnoses_all_code_cells_in_fixture() {
        let fixture_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple.ipynb");
        let text =
            std::fs::read_to_string(&fixture_path).expect("read tests/fixtures/simple.ipynb");

        let nb: nbformat::v4::Notebook =
            serde_json::from_str(&text).expect("fixture must parse as a valid v4 notebook");

        let code_cell_count = nb
            .cells
            .iter()
            .filter(|c| matches!(c, nbformat::v4::Cell::Code { .. }))
            .count();

        let mut cache = fresh_cache();
        let diags = compute_lsp_diagnostics(&text, &mut cache).unwrap();

        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("missing".to_string())))
            .collect();

        assert_eq!(
            missing.len(),
            code_cell_count,
            "expected one 'missing' diagnostic per code cell ({code_cell_count} total); \
             got {} — parsed cells: {:?}",
            missing.len(),
            nb.cells
                .iter()
                .enumerate()
                .map(|(i, c)| format!("[{i}] {:?}", c.cell_id()))
                .collect::<Vec<_>>(),
        );
    }
}
