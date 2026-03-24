use std::collections::HashMap;
use std::path::Path;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::ServiceExt,
    tool, tool_handler, tool_router,
    transport::io,
    ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};

use crate::diagnostics::{self, CellStatus, DiagnosticType};
use crate::diff_utils;
use crate::notebook::{load_notebook, CellExt};

// ---------------------------------------------------------------------------
// Tool parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RepairNotaBeneParams {
    /// Path to the .ipynb notebook file.
    pub notebook_path: String,
    /// Optional cell ID to target. If omitted, the first cell needing repair is used.
    pub cell_id: Option<String>,
    /// Per-diagnostic-type verbosity. Keys are diagnostic type names
    /// (missing, needs_review, ancestor_modified, diff_conflict, invalid_field).
    /// Values are "brief" or "detailed". Omitted keys default to "detailed".
    pub detail_level: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GenerateDiffParams {
    /// Path to the .ipynb notebook file.
    pub notebook_path: String,
    /// Cell ID to diff against.
    pub cell_id: String,
    /// The full intended cell source after patching. The tool computes a
    /// unified diff between the current cell source and this string.
    pub patched_source: String,
}

// ---------------------------------------------------------------------------
// MCP server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NotaBeneMcp {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl NotaBeneMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Inspect a notebook and return the first cell needing repair, \
        with full context and exact CLI commands to fix it. Call repeatedly in a loop until \
        all cells are valid. Use the optional cell_id to target a specific cell. \
        Use detail_level to pass {\"<diagnostic_type>\": \"brief\"} for types you have \
        already seen to reduce context. Run `nota-bene docs filters` for filter syntax \
        used in the update/accept/test commands this tool emits."
    )]
    async fn repair_nota_bene(
        &self,
        Parameters(params): Parameters<RepairNotaBeneParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = do_repair(&params).await;
        match result {
            Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            Err(e) => Err(McpError::internal_error(format!("{:#}", e), None)),
        }
    }

    #[tool(
        description = "Compute a unified diff between a notebook cell's current source and a \
        patched version you provide. Use this instead of writing diffs by hand — produce the \
        intended patched source, call this tool, then pass the returned diff string to \
        `nota-bene update`. Run `nota-bene docs filters` for filter syntax used in \
        update/accept/test commands."
    )]
    async fn generate_diff(
        &self,
        Parameters(params): Parameters<GenerateDiffParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = do_generate_diff(&params);
        match result {
            Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            Err(e) => Err(McpError::internal_error(format!("{:#}", e), None)),
        }
    }
}

#[tool_handler]
impl ServerHandler for NotaBeneMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_06_18)
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = NotaBeneMcp::new();
    let transport = io::stdio();
    let running = server.serve(transport).await?;
    running.waiting().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Core logic
// ---------------------------------------------------------------------------

fn do_generate_diff(params: &GenerateDiffParams) -> anyhow::Result<String> {
    let path = Path::new(&params.notebook_path);
    let nb = load_notebook(path)?;

    let cell = nb
        .cells
        .iter()
        .find(|c| matches!(c, nbformat::v4::Cell::Code { .. }) && c.cell_id_str() == params.cell_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Code cell `{}` not found in `{}`",
                params.cell_id,
                params.notebook_path
            )
        })?;

    let original = cell.source_str();
    let patched = &params.patched_source;

    match diff_utils::compute_diff(&original, patched) {
        None => Ok(
            "The patched source is identical to the current cell source. No diff needed.\n\
             You can omit the `diff` field in `nota-bene update`."
                .to_string(),
        ),
        Some(diff) => Ok(format!(
            "Unified diff for cell `{}` in `{}`:\n\n\
             ```diff\n{}\n```\n\n\
             Pass this diff string as the `\"diff\"` value in:\n\
             ```bash\n\
             nota-bene update {} --data '{{\"cell_id\": \"{}\", \"diff\": \"<diff above>\"}}'
             \n```",
            params.cell_id, params.notebook_path, diff, params.notebook_path, params.cell_id
        )),
    }
}

async fn do_repair(params: &RepairNotaBeneParams) -> anyhow::Result<String> {
    let path = Path::new(&params.notebook_path);
    let nb = load_notebook(path)?;

    let detail_map = params.detail_level.clone().unwrap_or_default();

    // Find all code cells with diagnostics.
    let cells = nb.cells.iter().enumerate().filter_map(|(idx, cell)| {
        if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
            return None;
        }
        let status = diagnostics::compute_cell_diagnostics(&nb, idx);
        if status.valid {
            return None;
        }
        Some((idx, cell, status))
    });

    // Pick the target cell.
    let target = if let Some(ref target_id) = params.cell_id {
        let mut found = None;
        for (idx, cell) in nb.cells.iter().enumerate() {
            if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
                continue;
            }
            if cell.cell_id_str() == target_id {
                let status = diagnostics::compute_cell_diagnostics(&nb, idx);
                if status.valid {
                    return Ok(format!(
                        "Cell `{}` in `{}` is already valid. Nothing to repair.",
                        target_id, params.notebook_path
                    ));
                }
                found = Some((idx, cell, status));
                break;
            }
        }
        found.ok_or_else(|| {
            anyhow::anyhow!(
                "Cell `{}` not found in `{}`",
                target_id,
                params.notebook_path
            )
        })?
    } else {
        let first = cells.into_iter().next();
        match first {
            Some(t) => t,
            None => {
                return Ok(format!(
                    "All cells in `{}` are valid. Nothing to repair.",
                    params.notebook_path
                ));
            }
        }
    };

    let (cell_index, cell, status) = target;
    let cell_id = cell.cell_id_str().to_string();
    let source = cell.source_str();
    let nb_data = cell.nota_bene();

    // Determine which diagnostic types are present.
    let diag_types: Vec<DiagnosticType> = status
        .diagnostics
        .iter()
        .map(|d| d.r#type.clone())
        .collect::<Vec<_>>();
    // Deduplicate.
    let mut diag_types_deduped: Vec<DiagnosticType> = Vec::new();
    for dt in &diag_types {
        if !diag_types_deduped.contains(dt) {
            diag_types_deduped.push(dt.clone());
        }
    }

    // Run tests if the cell has one.
    let test_result = if let Some(test) = nb_data.as_ref().and_then(|d| d.test.as_ref()) {
        run_test(&nb, cell_index, &cell_id, &test.name).await
    } else {
        "No test defined for this cell.".to_string()
    };

    // Build response.
    let mut out = String::new();

    // Header.
    let type_names: Vec<String> = diag_types_deduped
        .iter()
        .map(|d| format!("{}", d))
        .collect();
    out.push_str(&format!(
        "Cell `{}` at index {} in `{}` has diagnostics: {}\n\n",
        cell_id,
        cell_index,
        params.notebook_path,
        type_names.join(", ")
    ));

    // Workflow rule — stated upfront.
    out.push_str(
        "> **Workflow**: After making changes with `nota-bene update`, always run the \
         tests to verify the cell works correctly before accepting:\n\
         > ```bash\n",
    );
    out.push_str(&format!(
        "> nota-bene test {} --filter cell:{}\n",
        params.notebook_path, cell_id
    ));
    out.push_str("> ```\n");
    out.push_str(
        "> Only call `accept` once the tests pass and you are satisfied the cell is \
         fully ready. `accept` stamps the SHA snapshot — after this, the cell will no \
         longer be flagged. The accept command is:\n\
         > ```bash\n",
    );
    out.push_str(&format!(
        "> nota-bene accept {} --filter cell:{}\n",
        params.notebook_path, cell_id
    ));
    out.push_str("> ```\n\n");

    // Cell source.
    out.push_str("## Cell source\n\n```python\n");
    out.push_str(&source);
    if !source.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n\n");

    // Existing metadata (if any).
    if let Some(ref data) = nb_data {
        append_existing_metadata(&mut out, data);
    }

    // Test result.
    if nb_data.as_ref().and_then(|d| d.test.as_ref()).is_some() {
        out.push_str("## Test result\n\n");
        out.push_str(&test_result);
        out.push_str("\n\n");
    }

    // Per-diagnostic-type instructions.
    for dt in &diag_types_deduped {
        let dt_name = format!("{}", dt);
        let is_brief = detail_map
            .get(&dt_name)
            .map(|v| v == "brief")
            .unwrap_or(false);
        append_diagnostic_section(
            &mut out,
            dt,
            is_brief,
            &params.notebook_path,
            &cell_id,
            &nb_data,
            &test_result,
            &status,
        );
    }

    // Loop instruction.
    out.push_str("Then call `repair_nota_bene` again to continue to the next cell.\n");

    // Brief mode hint (only if everything was detailed).
    if !detail_map.values().any(|v| v == "brief") {
        out.push_str(&format!(
            "\nOn subsequent calls, you may pass `detail_level` with `\"brief\"` for \
             diagnostic types you've already seen, to reduce context. For example: \
             `detail_level: {{\"{}\": \"brief\"}}`\n",
            type_names.first().unwrap_or(&"missing".to_string())
        ));
    }

    out.push_str(
        "\nFor full filter syntax used in the commands above, run: \
         `nota-bene docs filters`\n",
    );

    Ok(out)
}

// ---------------------------------------------------------------------------
// Test runner (calls test_runner directly)
// ---------------------------------------------------------------------------

async fn run_test(
    nb: &nbformat::v4::Notebook,
    cell_index: usize,
    cell_id: &str,
    test_name: &str,
) -> String {
    // Serialize the test notebook outside the blocking task.
    let test_nb = match crate::test_runner::build_test_notebook(nb, cell_index) {
        Ok(nb) => nb,
        Err(e) => return format!("Failed to build test notebook: {e}"),
    };
    let test_nb_json = match serde_json::to_string(&test_nb) {
        Ok(s) => s,
        Err(e) => return format!("Failed to serialize test notebook: {e}"),
    };

    let cell_id = cell_id.to_string();
    let test_name = test_name.to_string();

    let result = tokio::task::spawn_blocking(move || {
        crate::test_runner::run_executor_subprocess(
            "python",
            "30",
            &test_nb_json,
            &cell_id,
            &test_name,
        )
    })
    .await;

    match result {
        Ok(r) => format_test_result(&r),
        Err(e) => format!("Test task panicked: {e}"),
    }
}

fn format_test_result(result: &crate::test_runner::CellTestResult) -> String {
    use crate::test_runner::CellTestResult;
    match result {
        CellTestResult::Completed { subtests, .. } => {
            let total = subtests.len();
            let passed = subtests.iter().filter(|s| s.passed).count();
            let failed = total - passed;
            let mut out = String::new();
            if failed == 0 {
                out.push_str(&format!("All {} subtests passed:", total));
                for s in subtests {
                    out.push_str(&format!("\n  - {} ✓", s.name));
                }
            } else {
                out.push_str(&format!(
                    "FAILED — {} of {} subtests failed:",
                    failed, total
                ));
                for s in subtests {
                    if s.passed {
                        out.push_str(&format!("\n  - {} ✓", s.name));
                    } else {
                        let err = s.error.as_deref().unwrap_or("(no detail)");
                        out.push_str(&format!("\n  - {} ✗ — {}", s.name, err));
                    }
                }
            }
            out
        }
        CellTestResult::Error { error, .. } => {
            let mut out = format!("ERROR in phase `{}`: {}", error.phase, error.detail);
            if let Some(ref tb) = error.traceback {
                out.push_str(&format!("\n\nTraceback:\n```\n{}\n```", tb));
            }
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Response building helpers
// ---------------------------------------------------------------------------

fn append_existing_metadata(out: &mut String, data: &crate::metadata::NotaBeneData) {
    // Fixtures.
    out.push_str("## Existing fixtures\n\n");
    if let Some(ref fixtures) = data.fixtures {
        if fixtures.is_empty() {
            out.push_str("None\n\n");
        } else {
            let json = serde_json::to_string_pretty(fixtures).unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("```json\n{}\n```\n\n", json));
        }
    } else {
        out.push_str("None\n\n");
    }

    // Diff.
    out.push_str("## Existing diff\n\n");
    if let Some(ref diff) = data.diff {
        out.push_str(&format!("```diff\n{}\n```\n\n", diff));
    } else {
        out.push_str("None\n\n");
    }

    // Test.
    out.push_str("## Existing test\n\n");
    if let Some(ref test) = data.test {
        out.push_str(&format!(
            "Name: {}\n```python\n{}\n```\n\n",
            test.name, test.source
        ));
    } else {
        out.push_str("None\n\n");
    }
}

#[allow(clippy::too_many_arguments)]
fn append_diagnostic_section(
    out: &mut String,
    dt: &DiagnosticType,
    is_brief: bool,
    notebook_path: &str,
    cell_id: &str,
    nb_data: &Option<crate::metadata::NotaBeneData>,
    test_result: &str,
    status: &CellStatus,
) {
    match dt {
        DiagnosticType::Missing => {
            append_missing(out, is_brief, notebook_path, cell_id, nb_data, test_result)
        }
        DiagnosticType::NeedsReview => {
            append_needs_review(out, is_brief, notebook_path, cell_id, test_result)
        }
        DiagnosticType::AncestorModified => {
            let ancestors: Vec<String> = status
                .diagnostics
                .iter()
                .filter(|d| d.r#type == DiagnosticType::AncestorModified)
                .map(|d| d.message.clone())
                .collect();
            append_ancestor_modified(
                out,
                is_brief,
                notebook_path,
                cell_id,
                test_result,
                &ancestors,
            )
        }
        DiagnosticType::DiffConflict => {
            let diff = nb_data
                .as_ref()
                .and_then(|d| d.diff.as_deref())
                .unwrap_or("(no diff stored)");
            append_diff_conflict(out, is_brief, notebook_path, cell_id, diff)
        }
        DiagnosticType::InvalidField => {
            let raw = nb_data
                .as_ref()
                .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
                .unwrap_or_default();
            append_invalid_field(out, is_brief, notebook_path, cell_id, &raw)
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostic-specific sections
// ---------------------------------------------------------------------------

fn append_missing(
    out: &mut String,
    is_brief: bool,
    notebook_path: &str,
    cell_id: &str,
    nb_data: &Option<crate::metadata::NotaBeneData>,
    test_result: &str,
) {
    // Determine what's actually present vs missing.
    let has_fixtures = nb_data
        .as_ref()
        .and_then(|d| d.fixtures.as_ref())
        .map(|f| !f.is_empty())
        .unwrap_or(false);
    let has_diff = nb_data.as_ref().and_then(|d| d.diff.as_ref()).is_some();
    let has_test = nb_data.as_ref().and_then(|d| d.test.as_ref()).is_some();
    let has_any_metadata = nb_data.is_some();

    // Sub-case: metadata exists (with at least a test) but was never accepted.
    // The cell is only flagged Missing because shas haven't been stamped yet.
    if has_any_metadata && has_test {
        let test_passed = test_result.starts_with("All ") || test_result.contains("passed");
        out.push_str("## Not yet accepted\n\n");
        out.push_str(
            "This cell has nota-bene metadata but has never been accepted. \
             The `shas` field is empty, so it will always appear as `missing` \
             until accepted.\n\n",
        );
        if test_passed {
            out.push_str("The test passes. Accept the cell to stamp it as valid:\n\n");
            append_accept_command(out, notebook_path, cell_id);
        } else {
            out.push_str(
                "The test is failing. Fix the metadata, re-run the test, then accept:\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        }
        return;
    }

    // Sub-case: has some metadata but is still incomplete (e.g. fixtures but no test).
    // List exactly what's missing.
    if has_any_metadata {
        out.push_str("## Incomplete metadata\n\n");
        out.push_str("This cell has some nota-bene metadata but is missing:\n");
        if !has_fixtures {
            out.push_str("- `fixtures` (if external resources or side effects are involved)\n");
        }
        if !has_diff {
            out.push_str("- `diff` (if the cell source needs patching to use fixtures)\n");
        }
        if !has_test {
            out.push_str("- `test` — **required**\n");
        }
        out.push('\n');
    } else {
        out.push_str("## Missing metadata\n\n");
        out.push_str("This cell has no nota-bene metadata at all.\n\n");
    }

    if is_brief {
        out.push_str(
            "Create the missing fields. Fixtures are needed only if the cell has \
             external resources or side effects. A diff is needed only to reroute \
             the cell to use fixtures.\n\n",
        );
        append_update_command_template(out, notebook_path, cell_id);
        out.push_str("\nThen run the test to confirm it passes:\n\n");
        append_test_command(out, notebook_path, cell_id);
        out.push_str("Once passing, accept:\n\n");
        append_accept_command(out, notebook_path, cell_id);
        out.push_str(
            "Call with `detail_level: {\"missing\": \"detailed\"}` for full instructions.\n\n",
        );
    } else {
        out.push_str(
            "nota-bene attaches test infrastructure to each cell so that hundreds of \
             parallel kernels can quickly recreate the notebook state up to any cell \
             and run isolated tests.\n\n",
        );
        if !has_fixtures {
            out.push_str("### Fixtures\n\n");
            out.push_str(
                "Fixtures create deterministic mock data and controlled side effects that \
                 replace real resources (files, databases, APIs). They must be lightweight \
                 (KB not GB) so test kernels spin up fast.\n\n\
                 A fixture is NOT needed if:\n\
                 - The cell's data was already initialized by a previous cell\n\
                 - The cell has no external side effects or resource dependencies\n\n\
                 Each fixture has a name (the key), and an object with:\n\
                 - `description`: What this fixture sets up and why\n\
                 - `priority`: Integer (lower runs first)\n\
                 - `source`: Python code that creates the mock data/resources\n\n",
            );
        }
        if !has_diff {
            out.push_str("### Diff\n\n");
            out.push_str(
                "A minimal patch that reroutes the cell to use fixtures instead of real \
                 resources. For example, replacing a CSV file path with a temp file created \
                 by the fixture. The diff should change as little as possible — the goal is \
                 to test the user's actual code, not rewritten code. A diff is NOT needed if \
                 the cell doesn't reference any external resources that fixtures replace.\n\n\
                 Do NOT write a unified diff by hand. Instead:\n\
                 1. Produce the full patched cell source with your intended changes\n\
                 2. Call `generate_diff` with that patched source to get the correct diff\n\
                 3. Use the returned diff string in `nota-bene update`\n\n",
            );
        }
        if !has_test {
            out.push_str("### Test\n\n");
            out.push_str(
                "Python code with subtests that verify the cell behaves correctly. Use \
                 `nota_bene.subtest(\"name\")` as a context manager to define each subtest. \
                 Each subtest should cover a specific condition or edge case the cell handles.\n\n",
            );
        }
        out.push_str("### Commands\n\n");
        append_update_command_template(out, notebook_path, cell_id);
        out.push_str(
            "\nIf no fixtures or diff are needed, omit those fields entirely and only \
             provide the test.\n\n\
             Then run the test to confirm it passes:\n\n",
        );
        append_test_command(out, notebook_path, cell_id);
        out.push_str("Once the test passes, accept the cell:\n\n");
        append_accept_command(out, notebook_path, cell_id);
    }
}

fn append_needs_review(
    out: &mut String,
    is_brief: bool,
    notebook_path: &str,
    cell_id: &str,
    test_result: &str,
) {
    let test_passed = test_result.starts_with("All ") || test_result.contains("passed");

    if is_brief {
        out.push_str("## Needs review\n\n");
        if test_passed {
            out.push_str(
                "Test passes. If the metadata still looks correct for the current source, \
                 accept. If you make any changes, re-run the test before accepting.\n\n",
            );
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        } else {
            out.push_str(
                "Test failing. Update fixtures/diff/test, re-run the test, then accept \
                 once passing.\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        }
        out.push_str(
            "Call with `detail_level: {\"needs_review\": \"detailed\"}` for full instructions.\n\n",
        );
    } else {
        out.push_str("## Needs review — instructions\n\n");
        out.push_str(
            "The cell source or metadata has changed since the last accept. Review \
             whether the existing fixtures, diff, and test still make sense for the \
             current cell source.\n\n\
             Remember:\n\
             - Fixtures provide deterministic mock data (KB not GB)\n\
             - The diff should be minimal — only reroute to use fixtures\n\
             - Tests should cover the specific behaviors of THIS cell\n\n",
        );

        if test_passed {
            out.push_str(
                "The test still passes. If the metadata looks correct for the current \
                 source, accept the cell:\n\n",
            );
            append_accept_command(out, notebook_path, cell_id);
            out.push_str(
                "If you need to update the metadata first, make your changes, then \
                 re-run the test to confirm it still passes before accepting:\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        } else {
            out.push_str(
                "The test is failing. Update the metadata as needed, re-run the test \
                 to confirm it passes, then accept:\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        }
    }
}

fn append_ancestor_modified(
    out: &mut String,
    is_brief: bool,
    notebook_path: &str,
    cell_id: &str,
    test_result: &str,
    ancestor_messages: &[String],
) {
    let test_passed = test_result.starts_with("All ") || test_result.contains("passed");

    out.push_str("## Ancestor modified\n\n");
    out.push_str("Changed ancestors:\n");
    for msg in ancestor_messages {
        out.push_str(&format!("- {}\n", msg));
    }
    out.push('\n');

    if is_brief {
        if test_passed {
            out.push_str(
                "Test passes. If the ancestor changes don't affect this cell, accept. \
                 If you make any changes, re-run the test before accepting.\n\n",
            );
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        } else {
            out.push_str(
                "Test failing. Update metadata to account for upstream changes, \
                 re-run the test, then accept once passing.\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        }
        out.push_str("Call with `detail_level: {\"ancestor_modified\": \"detailed\"}` for full instructions.\n\n");
    } else {
        out.push_str(
            "Preceding cells have changed, which may affect this cell's execution \
             context. The fixtures and diff may need updating if the upstream data \
             shape or control flow changed.\n\n",
        );
        if test_passed {
            out.push_str(
                "The test still passes despite ancestor changes. If the upstream changes \
                 don't affect this cell's behavior, accept it:\n\n",
            );
            append_accept_command(out, notebook_path, cell_id);
            out.push_str(
                "If you need to update the metadata to account for upstream changes, \
                 do so, then re-run the test to confirm it passes before accepting:\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        } else {
            out.push_str(
                "The test is failing, likely due to changes in preceding cells. Update \
                 the fixtures or test to account for the new upstream state, re-run the \
                 test to confirm it passes, then accept:\n\n",
            );
            append_update_command_existing(out, notebook_path, cell_id);
            append_test_command(out, notebook_path, cell_id);
            append_accept_command(out, notebook_path, cell_id);
        }
    }
}

fn append_diff_conflict(
    out: &mut String,
    is_brief: bool,
    notebook_path: &str,
    cell_id: &str,
    broken_diff: &str,
) {
    out.push_str("## Diff conflict\n\n");
    out.push_str(&format!(
        "Stored diff (broken):\n```diff\n{}\n```\n\n",
        broken_diff
    ));

    if is_brief {
        out.push_str(
            "Produce the patched cell source with your intended changes, call \
             `generate_diff` to get the correct diff, then update. Or remove the diff \
             if no longer needed.\n\n",
        );
    } else {
        out.push_str(
            "The cell source has changed and the stored diff can no longer be applied. \
             The diff's only purpose is to reroute code to use fixture-provided \
             resources (e.g., swap a file path, redirect an API call to a mock). \
             It should change as little as possible.\n\n\
             Do NOT write a unified diff by hand. Instead:\n\
             1. Produce the full patched cell source with your intended changes\n\
             2. Call `generate_diff` to compute the correct unified diff\n\
             3. Pass the returned diff string to `nota-bene update`\n\n\
             Or set `diff` to null if a diff is no longer needed.\n\n",
        );
    }

    out.push_str(&format!(
        "```bash\nnota-bene update {} --data '{{\n  \"cell_id\": \"{}\",\n  \"diff\": \"<output of generate_diff, or null to remove>\"\n}}'\n```\n\n",
        notebook_path, cell_id
    ));
    out.push_str("Then run the test to confirm it passes:\n\n");
    append_test_command(out, notebook_path, cell_id);
    out.push_str("Once passing, accept:\n\n");
    append_accept_command(out, notebook_path, cell_id);

    if is_brief {
        out.push_str("Call with `detail_level: {\"diff_conflict\": \"detailed\"}` for full instructions.\n\n");
    }
}

fn append_invalid_field(
    out: &mut String,
    is_brief: bool,
    notebook_path: &str,
    cell_id: &str,
    raw_metadata: &str,
) {
    out.push_str("## Invalid field\n\n");
    out.push_str(&format!(
        "Current metadata (raw):\n```json\n{}\n```\n\n",
        raw_metadata
    ));

    if is_brief {
        out.push_str("Fix the invalid fields.\n\n");
    } else {
        out.push_str(
            "One or more nota-bene metadata fields have validation errors. Fix the \
             invalid fields using the update command.\n\n\
             Valid field formats:\n\
             - `fixtures`: object keyed by name, each with `description` (string), \
               `priority` (integer), `source` (string)\n\
             - `diff`: string (unified diff) or null\n\
             - `test`: object with `name` (string) and `source` (string), or null\n\n",
        );
    }

    append_update_command_existing(out, notebook_path, cell_id);
    out.push_str("Then run the test to confirm it passes:\n\n");
    append_test_command(out, notebook_path, cell_id);
    out.push_str("Once passing, accept:\n\n");
    append_accept_command(out, notebook_path, cell_id);

    if is_brief {
        out.push_str(
            "Call with `detail_level: {\"invalid_field\": \"detailed\"}` for format reference.\n\n",
        );
    }
}

// ---------------------------------------------------------------------------
// CLI command templates
// ---------------------------------------------------------------------------

fn append_update_command_template(out: &mut String, notebook_path: &str, cell_id: &str) {
    out.push_str(&format!(
        "```bash\nnota-bene update {} --data '{{\n\
         \x20 \"cell_id\": \"{}\",\n\
         \x20 \"fixtures\": {{\n\
         \x20   \"<fixture_name>\": {{\n\
         \x20     \"description\": \"<what this fixture sets up>\",\n\
         \x20     \"priority\": 0,\n\
         \x20     \"source\": \"<python code>\"\n\
         \x20   }}\n\
         \x20 }},\n\
         \x20 \"diff\": \"<unified diff string, or omit entirely if not needed>\",\n\
         \x20 \"test\": {{\n\
         \x20   \"name\": \"<descriptive_test_name>\",\n\
         \x20   \"source\": \"<python test code using nota_bene.subtest()>\"\n\
         \x20 }}\n\
         }}'\n```\n",
        notebook_path, cell_id
    ));
}

fn append_update_command_existing(out: &mut String, notebook_path: &str, cell_id: &str) {
    out.push_str(&format!(
        "```bash\nnota-bene update {} --data '{{\n\
         \x20 \"cell_id\": \"{}\",\n\
         \x20 <only include fields that need changing>\n\
         }}'\n```\n\n",
        notebook_path, cell_id
    ));
}

fn append_test_command(out: &mut String, notebook_path: &str, cell_id: &str) {
    out.push_str(&format!(
        "```bash\nnota-bene test {} --filter cell:{}\n```\n\n",
        notebook_path, cell_id
    ));
}

fn append_accept_command(out: &mut String, notebook_path: &str, cell_id: &str) {
    out.push_str(&format!(
        "```bash\nnota-bene accept {} --filter cell:{}\n```\n\n",
        notebook_path, cell_id
    ));
}
