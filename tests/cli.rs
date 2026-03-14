mod common;

use std::fs;
use std::path::Path;
use std::process::Stdio;

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Copy a fixture notebook into a temp directory and return both.
fn setup_fixture(name: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("create tempdir");
    let dest = dir.path().join(name);
    fs::copy(common::fixtures_dir().join(name), &dest)
        .unwrap_or_else(|e| panic!("copy fixture {name}: {e}"));
    (dir, dest)
}

/// Run `nota-bene edit <path>` (non-blocking — creates the editor notebook and
/// exits immediately). Returns the exit status.
fn run_edit(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args(["edit", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn nota-bene edit")
}

/// Run `nota-bene edit --continue <path>`. Returns the exit status.
fn run_edit_continue(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args(["edit", "--continue", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn nota-bene edit --continue")
}

/// Run `nota-bene edit --continue --force <path>`. Returns the exit status.
fn run_edit_continue_force(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args([
            "edit",
            "--continue",
            "--force",
            source_path.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn nota-bene edit --continue --force")
}

/// Run `nota-bene edit --clean <path>`. Returns the exit status.
fn run_edit_clean(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args(["edit", "--clean", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn nota-bene edit --clean")
}

/// Run the full edit → modify → continue workflow:
///   1. `nota-bene edit <path>` — creates editor notebook and exits.
///   2. Call `modify` with the editor notebook path.
///   3. `nota-bene edit --continue <path>` — applies changes.
///
/// Returns the `--continue` exit status.
fn run_edit_with_modifications<F>(source_path: &Path, modify: F) -> std::process::ExitStatus
where
    F: FnOnce(&Path),
{
    let stem = source_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let editor_path = source_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{}.nota-bene.ipynb", stem));

    let edit_status = run_edit(source_path);
    assert!(
        edit_status.success(),
        "nota-bene edit exited non-zero during setup"
    );
    assert!(
        editor_path.exists(),
        "editor notebook not created at {editor_path:?}"
    );

    modify(&editor_path);

    run_edit_continue(source_path)
}

/// Execute a notebook via `tests/execute_nb.py` using the test venv's Python,
/// writing outputs to `output_path`. Returns the exit status.
fn execute_notebook(
    nb_path: &std::path::Path,
    output_path: &std::path::Path,
) -> std::process::ExitStatus {
    std::process::Command::new(common::python())
        .args([
            common::execute_nb_script().to_str().unwrap(),
            nb_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
        ])
        .status()
        .expect("execute notebook")
}

/// Collect all stream-output text from a parsed notebook JSON value.
fn stream_outputs(nb: &serde_json::Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(cells) = nb["cells"].as_array() {
        for cell in cells {
            if let Some(outputs) = cell["outputs"].as_array() {
                for output in outputs {
                    if output["output_type"].as_str() == Some("stream") {
                        if let Some(s) = output["text"].as_str() {
                            texts.push(s.to_string());
                        } else if let Some(arr) = output["text"].as_array() {
                            texts.push(
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join(""),
                            );
                        }
                    }
                }
            }
        }
    }
    texts
}

/// Return true if any cell in the notebook JSON has an error output.
fn has_error_output(nb: &serde_json::Value) -> bool {
    nb["cells"].as_array().map_or(false, |cells| {
        cells.iter().any(|cell| {
            cell["outputs"].as_array().map_or(false, |outputs| {
                outputs
                    .iter()
                    .any(|o| o["output_type"].as_str() == Some("error"))
            })
        })
    })
}

/// Return true if any cell in the notebook JSON has `nota-bene.editor` metadata.
fn has_editor_metadata(nb: &serde_json::Value) -> bool {
    nb["cells"].as_array().map_or(false, |cells| {
        cells.iter().any(|cell| {
            cell["metadata"]
                .get("nota-bene")
                .map_or(false, |nb_meta| nb_meta.get("editor").is_some())
        })
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full smoke test:
///   1. Run `nota-bene edit` on simple.ipynb — exits immediately.
///   2. Copy the editor notebook so we can execute it.
///   3. Run `nota-bene edit --continue` — applies changes.
///   4. Execute the copy via nbclient.
///   5. Validate cell outputs.
///   6. Validate the source notebook was saved back without editor metadata.
///   7. Validate the editor notebook was deleted after a successful apply.
#[test]
fn smoke_edit_executes_and_saves_cleanly() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.nota-bene.ipynb");
    let copy_path = dir.path().join("editor_copy.ipynb");
    let output_path = dir.path().join("output.ipynb");

    // --- edit step ---
    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "nota-bene edit exited non-zero");
    assert!(editor_path.exists(), "editor notebook not created");

    // Copy the editor notebook before --continue deletes it.
    fs::copy(&editor_path, &copy_path).expect("copy editor notebook for execution");

    // --- continue step ---
    let continue_status = run_edit_continue(&source_path);
    assert!(
        continue_status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    // --- validate the editor file was cleaned up ---
    assert!(
        !editor_path.exists(),
        "editor notebook was not cleaned up after successful --continue"
    );

    // --- execute step ---
    let exec_status = execute_notebook(&copy_path, &output_path);
    assert!(
        exec_status.success(),
        "notebook execution failed — check {output_path:?} for details"
    );

    // --- validate outputs ---
    let output_nb: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&output_path).expect("read output notebook"))
            .expect("parse output notebook");

    assert!(
        !has_error_output(&output_nb),
        "one or more cells produced an error output"
    );

    let outputs = stream_outputs(&output_nb);
    let all_output = outputs.join("\n");

    assert!(
        all_output.contains("cell is skipped") || all_output.contains("Skipped"),
        "expected '%%nb_skip' to produce skip output but got:\n{all_output}"
    );
    assert!(
        all_output.contains("check_total"),
        "expected skipped output to mention 'check_total' but got:\n{all_output}"
    );

    // --- validate source notebook was written back without editor metadata ---
    let saved_nb: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&source_path).expect("read saved source notebook"),
    )
    .expect("parse saved source notebook");

    assert!(
        !has_editor_metadata(&saved_nb),
        "source notebook still contains nota-bene.editor metadata after save"
    );
}

/// Attempting to edit when the editor file already exists must fail immediately
/// with a message suggesting --continue or --clean, and leave the source notebook untouched.
#[test]
fn edit_fails_if_editor_file_already_exists() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.nota-bene.ipynb");

    // Pre-create the editor file.
    fs::write(&editor_path, b"{}").expect("create dummy editor file");

    let source_before = fs::read(&source_path).expect("read source before");

    let output = std::process::Command::new(common::binary())
        .args(["edit", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .expect("spawn nota-bene");

    assert!(
        !output.status.success(),
        "expected non-zero exit when editor file already exists"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--continue") || stderr.contains("--clean"),
        "expected error message to suggest --continue or --clean, got:\n{stderr}"
    );

    let source_after = fs::read(&source_path).expect("read source after");
    assert_eq!(
        source_before, source_after,
        "source notebook was modified despite error"
    );
}

/// `edit --clean` deletes the editor notebook and recreates it fresh.
#[test]
fn edit_clean_deletes_editor_file() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.nota-bene.ipynb");

    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "nota-bene edit exited non-zero");
    assert!(editor_path.exists(), "editor notebook not created");

    let clean_status = run_edit_clean(&source_path);
    assert!(
        clean_status.success(),
        "nota-bene edit --clean exited non-zero"
    );
    assert!(
        editor_path.exists(),
        "editor notebook does not exist after --clean (should be recreated)"
    );
}

/// `edit --clean` when no editor file exists succeeds silently and still creates a fresh one.
#[test]
fn edit_clean_fails_if_no_editor_file() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.nota-bene.ipynb");

    let status = run_edit_clean(&source_path);
    assert!(
        status.success(),
        "expected zero exit from --clean when no editor file exists"
    );
    assert!(
        editor_path.exists(),
        "editor notebook should be created by --clean even when none existed"
    );
}

/// `edit --continue` when no editor file exists must fail with an error.
#[test]
fn edit_continue_fails_if_no_editor_file() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");

    let status = run_edit_continue(&source_path);
    assert!(
        !status.success(),
        "expected non-zero exit from --continue when no editor file exists"
    );
}

// ---------------------------------------------------------------------------
// Round-trip and simulation tests
// ---------------------------------------------------------------------------

/// Helper: extract the `nota-bene` metadata object from a cell identified by
/// its `id` field. Returns `serde_json::Value::Null` if not found.
fn cell_nb_meta<'a>(nb: &'a serde_json::Value, cell_id: &str) -> &'a serde_json::Value {
    if let Some(cells) = nb["cells"].as_array() {
        for cell in cells {
            if cell["id"].as_str() == Some(cell_id) {
                let meta = &cell["metadata"]["nota-bene"];
                if !meta.is_null() {
                    return meta;
                }
            }
        }
    }
    &serde_json::Value::Null
}

/// After a round-trip with no user modifications the `nota-bene` metadata on
/// every source cell must preserve the original non-shas fields.
/// SHA snapshots are now stamped inside `apply_editor_to_source` for every cell
/// that goes through the editor, so a new `shas` field will be present — that
/// is expected and correct.
#[test]
fn round_trip_no_changes_preserves_metadata() {
    let (dir, source_path) = setup_fixture("simple.ipynb");

    let original: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read original"))
            .expect("parse original");

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    for cell_id in &["plain-data", "compute-total"] {
        let before = cell_nb_meta(&original, cell_id);
        let after = cell_nb_meta(&saved, cell_id);

        // SHA snapshots are now stamped by apply_editor_to_source on --continue.
        // Strip `shas` from `after` before comparing with `before`.
        let mut after_without_shas = after.clone();
        if let Some(obj) = after_without_shas.as_object_mut() {
            obj.remove("shas");
        }

        assert_eq!(
            before, &after_without_shas,
            "nota-bene metadata changed for cell '{cell_id}' despite no user edits \
             (ignoring newly-stamped shas)"
        );
    }
    assert!(
        !dir.path().join("simple.nota-bene.ipynb").exists(),
        "editor notebook not deleted after successful --continue"
    );
}

/// Editing the `# fixture:` comment renames the fixture in the source notebook.
#[test]
fn edit_rename_fixture_updates_source() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |editor_path| {
        let raw = fs::read_to_string(editor_path).expect("read editor notebook");
        let mut nb: serde_json::Value = serde_json::from_str(&raw).expect("parse editor notebook");

        if let Some(cells) = nb["cells"].as_array_mut() {
            for cell in cells.iter_mut() {
                let role = cell["metadata"]["nota-bene"]["editor"]["role"]
                    .as_str()
                    .unwrap_or("");
                let cell_id = cell["metadata"]["nota-bene"]["editor"]["cell_id"]
                    .as_str()
                    .unwrap_or("");
                if role == "fixture" && cell_id == "compute-total" {
                    if let Some(src) = cell["source"].as_str() {
                        cell["source"] = serde_json::Value::String(
                            src.replace("# fixture: setup_data", "# fixture: renamed_fixture"),
                        );
                    } else if let Some(arr) = cell["source"].as_array() {
                        let joined = arr
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("");
                        cell["source"] = serde_json::Value::String(
                            joined.replace("# fixture: setup_data", "# fixture: renamed_fixture"),
                        );
                    }
                    break;
                }
            }
        }

        fs::write(
            editor_path,
            serde_json::to_string_pretty(&nb).expect("serialize"),
        )
        .expect("write modified editor notebook");
    });
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    let fixtures = &cell_nb_meta(&saved, "compute-total")["fixtures"];
    assert!(
        fixtures.get("renamed_fixture").is_some(),
        "expected 'renamed_fixture' in fixtures, got: {fixtures}"
    );
    assert!(
        fixtures.get("setup_data").is_none(),
        "old fixture name 'setup_data' still present: {fixtures}"
    );
}

/// Modifying the patched-source cell content causes a `diff` to be written
/// back into the source notebook's cell metadata.
#[test]
fn edit_modified_source_writes_diff() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |editor_path| {
        let raw = fs::read_to_string(editor_path).expect("read editor notebook");
        let mut nb: serde_json::Value = serde_json::from_str(&raw).expect("parse editor notebook");

        if let Some(cells) = nb["cells"].as_array_mut() {
            for cell in cells.iter_mut() {
                let role = cell["metadata"]["nota-bene"]["editor"]["role"]
                    .as_str()
                    .unwrap_or("");
                let cell_id = cell["metadata"]["nota-bene"]["editor"]["cell_id"]
                    .as_str()
                    .unwrap_or("");
                if role == "patched-source" && cell_id == "compute-total" {
                    cell["source"] = serde_json::Value::String("total = sum(data) * 2".to_string());
                    break;
                }
            }
        }

        fs::write(
            editor_path,
            serde_json::to_string_pretty(&nb).expect("serialize"),
        )
        .expect("write modified editor notebook");
    });
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    let diff = &cell_nb_meta(&saved, "compute-total")["diff"];
    assert!(
        !diff.is_null(),
        "expected a diff to be written after source modification, got null"
    );
    assert!(
        diff.as_str().is_some(),
        "expected diff to be a string, got: {diff}"
    );
}

/// Adding an untagged code cell immediately before the patched-source cell
/// (simulating a user inserting a new fixture) causes a new fixture entry to
/// appear in the source notebook's cell metadata.
#[test]
fn edit_add_fixture_by_position() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |editor_path| {
        let raw = fs::read_to_string(editor_path).expect("read editor notebook");
        let mut nb: serde_json::Value = serde_json::from_str(&raw).expect("parse editor notebook");

        let cells = nb["cells"].as_array_mut().expect("cells is an array");
        let insert_idx = cells
            .iter()
            .position(|c| {
                c["metadata"]["nota-bene"]["editor"]["role"].as_str() == Some("patched-source")
                    && c["metadata"]["nota-bene"]["editor"]["cell_id"].as_str()
                        == Some("compute-total")
            })
            .expect("patched-source cell for compute-total not found");

        let new_cell = serde_json::json!({
            "cell_type": "code",
            "execution_count": null,
            "id": "new-fixture-cell",
            "metadata": {},
            "outputs": [],
            "source": "# fixture: extra_fixture\ndata = [100, 200]"
        });

        cells.insert(insert_idx, new_cell);

        fs::write(
            editor_path,
            serde_json::to_string_pretty(&nb).expect("serialize"),
        )
        .expect("write modified editor notebook");
    });
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    let fixtures = &cell_nb_meta(&saved, "compute-total")["fixtures"];
    assert!(
        fixtures.get("extra_fixture").is_some(),
        "expected new 'extra_fixture' to be present in fixtures, got: {fixtures}"
    );
    assert!(
        fixtures.get("setup_data").is_some(),
        "original 'setup_data' fixture was lost: {fixtures}"
    );
}

/// A cell with explicit `null` values for `fixtures`, `diff`, and `test`
/// no longer preserves those as JSON nulls after a round-trip — in the new
/// simplified model, `None` is serialized as an absent key, not null.
/// The nota-bene key itself is preserved (since the cell already had it).
#[test]
fn edit_explicit_nulls_become_absent_after_round_trip() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let original: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read original"))
            .expect("parse original");
    let orig_meta = cell_nb_meta(&original, "reviewed-pass");
    assert!(
        orig_meta
            .get("fixtures")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "fixture precondition: 'reviewed-pass' should have explicit null fixtures"
    );

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    let meta = cell_nb_meta(&saved, "reviewed-pass");

    // The nota-bene key itself must still exist (cell already had it).
    assert!(
        !meta.is_null(),
        "nota-bene key was removed from 'reviewed-pass'"
    );

    // In the new model, null fields are absent (not written as null).
    for key in &["fixtures", "diff", "test"] {
        assert!(
            meta.get(key).is_none(),
            "key '{key}' should be absent in 'reviewed-pass' after round-trip, got: {}",
            meta[key]
        );
    }
}

/// The section header for a stale cell must contain "Needs review" and
/// the staleness reason. `compute-total` in simple.ipynb has nota-bene
/// metadata but no `shas` entry → reason is "No staleness data".
#[test]
fn edit_stale_cell_header_contains_needs_review_and_reason() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.nota-bene.ipynb");

    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "nota-bene edit exited non-zero");

    let raw = fs::read_to_string(&editor_path).expect("read editor notebook");
    let nb: serde_json::Value = serde_json::from_str(&raw).expect("parse editor notebook");

    // Find the section-header cell for "compute-total".
    let header_src = nb["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .find(|c| {
            c["metadata"]["nota-bene"]["editor"]["role"].as_str() == Some("section-header")
                && c["metadata"]["nota-bene"]["editor"]["cell_id"].as_str() == Some("compute-total")
        })
        .map(|c| {
            let src = &c["source"];
            if let Some(s) = src.as_str() {
                s.to_string()
            } else if let Some(arr) = src.as_array() {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            } else {
                String::new()
            }
        })
        .expect("section-header for compute-total not found");

    assert!(
        header_src.contains("Needs review"),
        "expected 'Needs review' in section header but got:\n{header_src}"
    );
    assert!(
        header_src.to_lowercase().contains("shas missing")
            || header_src.to_lowercase().contains("staleness data"),
        "expected staleness reason in section header but got:\n{header_src}"
    );
}

/// After `edit --continue`, the test source stored in the notebook must not
/// begin with `%%nb_skip` — it should have been stripped during apply.
#[test]
fn edit_continue_strips_nb_skip_from_test_source() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    // Check that no cell's test.source starts with %%nb_skip.
    if let Some(cells) = saved["cells"].as_array() {
        for cell in cells {
            let test_src = &cell["metadata"]["nota-bene"]["test"]["source"];
            if let Some(s) = test_src.as_str() {
                assert!(
                    !s.starts_with("%%nb_skip"),
                    "test source still contains %%nb_skip after --continue: {s}"
                );
            } else if let Some(arr) = test_src.as_array() {
                let joined = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                assert!(
                    !joined.starts_with("%%nb_skip"),
                    "test source still contains %%nb_skip after --continue: {joined}"
                );
            }
        }
    }
}

#[test]
fn edit_continue_fails_on_source_conflict() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    // Create the editor notebook.
    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "nota-bene edit exited non-zero");

    // Modify the source notebook (simulate an external change).
    let raw = fs::read_to_string(&source_path).expect("read source");
    let mut nb: serde_json::Value = serde_json::from_str(&raw).expect("parse source");
    if let Some(cells) = nb["cells"].as_array_mut() {
        for cell in cells.iter_mut() {
            if cell["id"].as_str() == Some("compute-total") {
                cell["source"] =
                    serde_json::Value::String("total = 999  # externally modified".to_string());
                break;
            }
        }
    }
    fs::write(
        &source_path,
        serde_json::to_string_pretty(&nb).expect("serialize"),
    )
    .expect("write modified source");

    // --continue should detect the conflict and fail.
    let continue_status = run_edit_continue(&source_path);
    assert!(
        !continue_status.success(),
        "expected non-zero exit from --continue when source has changed"
    );
}

/// `edit --continue --force` succeeds even when the source has changed,
/// stripping all nota-bene metadata and applying the editor notebook.
#[test]
fn edit_continue_force_succeeds_despite_conflict() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "nota-bene edit exited non-zero");

    // Modify the source notebook.
    let raw = fs::read_to_string(&source_path).expect("read source");
    let mut nb: serde_json::Value = serde_json::from_str(&raw).expect("parse source");
    if let Some(cells) = nb["cells"].as_array_mut() {
        for cell in cells.iter_mut() {
            if cell["id"].as_str() == Some("compute-total") {
                cell["source"] =
                    serde_json::Value::String("total = 999  # externally modified".to_string());
                break;
            }
        }
    }
    fs::write(
        &source_path,
        serde_json::to_string_pretty(&nb).expect("serialize"),
    )
    .expect("write modified source");

    let force_status = run_edit_continue_force(&source_path);
    assert!(
        force_status.success(),
        "expected success from --continue --force despite conflict"
    );
}

/// After `edit --continue`, cells that went through the editor and have
/// nota-bene metadata must have a `shas` snapshot stamped on them.
/// `compute-total` and `reviewed-pass` both have nota-bene in simple.ipynb.
#[test]
fn edit_continue_stamps_shas_on_nota_bene_cells() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    for cell_id in &["compute-total", "reviewed-pass"] {
        let meta = cell_nb_meta(&saved, cell_id);
        assert!(
            !meta.is_null(),
            "cell '{cell_id}' has no nota-bene metadata"
        );
        let shas = meta.get("shas");
        assert!(
            shas.is_some() && shas.unwrap().is_array(),
            "cell '{cell_id}' is missing shas after --continue; meta: {meta}"
        );
        let shas_arr = shas.unwrap().as_array().unwrap();
        assert!(
            !shas_arr.is_empty(),
            "cell '{cell_id}' has an empty shas array after --continue"
        );
    }
}

/// After `edit --continue`, cells without nota-bene metadata must NOT have
/// shas stamped on them — only cells that actually went through the editor
/// with nota-bene are touched. `plain-data` has no nota-bene in simple.ipynb.
#[test]
fn edit_continue_does_not_stamp_shas_on_plain_cells() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(
        status.success(),
        "nota-bene edit --continue exited non-zero"
    );

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    // plain-data has no nota-bene metadata — its nota-bene key must remain absent.
    let meta = cell_nb_meta(&saved, "plain-data");
    assert!(
        meta.is_null(),
        "plain-data should have no nota-bene metadata after --continue, got: {meta}"
    );
}

// ===========================================================================
// nb view tests
// ===========================================================================

/// Run `nota-bene view <args>` and return (stdout, stderr, exit status).
fn run_view(source_path: &Path, extra_args: &[&str]) -> (String, String, std::process::ExitStatus) {
    let mut cmd = std::process::Command::new(common::binary());
    cmd.arg("view").arg(source_path);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn nota-bene view");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status)
}

#[test]
fn view_all_cells_returns_json_array() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, status) = run_view(&source_path, &[]);
    assert!(status.success(), "exit status should be 0");
    let arr: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(arr.is_array(), "output should be a JSON array");
}

#[test]
fn view_all_cells_excludes_markdown_cells() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &[]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    // No cell should have id "md-intro" (markdown cell)
    for cell in &arr {
        assert_ne!(
            cell["cell_id"].as_str(),
            Some("md-intro"),
            "markdown cells must be excluded"
        );
    }
}

#[test]
fn view_all_cells_count() {
    // simple.ipynb has 4 code cells and 1 markdown cell → 4 in output
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &[]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 4, "expected 4 code cells");
}

#[test]
fn view_cell_object_has_required_fields() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &[]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    for cell in &arr {
        assert!(cell.get("cell_id").is_some(), "cell_id missing");
        assert!(cell.get("source").is_some(), "source missing");
        assert!(cell.get("status").is_some(), "status missing");
    }
}

#[test]
fn view_filter_by_cell_id() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, status) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    assert!(status.success());
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["cell_id"].as_str(), Some("compute-total"));
}

#[test]
fn view_filter_by_cell_id_or() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(
        &source_path,
        &["--filter", "cell:compute-total,reviewed-pass"],
    );
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 2);
}

#[test]
fn view_filter_by_index_exact() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // Index is 0-based across *all* cells (including markdown).
    // Cell at index 0 is "md-intro" (markdown, excluded from output).
    // Index 1 is "plain-data" (first code cell).
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "index:1"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["cell_id"].as_str(), Some("plain-data"));
}

#[test]
fn view_filter_by_index_range() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // Indices 1..2 — plain-data and compute-total
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "index:1..2"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 2);
}

#[test]
fn view_filter_test_null() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // plain-data, reviewed-pass, and the empty cell have no test
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "test:null"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    for cell in &arr {
        assert_ne!(
            cell["cell_id"].as_str(),
            Some("compute-total"),
            "compute-total has a test and should be filtered out"
        );
    }
}

#[test]
fn view_filter_test_not_null() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "test:not null"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["cell_id"].as_str(), Some("compute-total"));
}

#[test]
fn view_and_filters() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // compute-total has a test AND fixtures; reviewed-pass has no test
    let (stdout, _stderr, _) = run_view(
        &source_path,
        &["--filter", "test:not null", "--filter", "fixtures:not null"],
    );
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["cell_id"].as_str(), Some("compute-total"));
}

#[test]
fn view_fields_projection() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &["--fields", "cell_id,source"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    for cell in &arr {
        assert!(cell.get("cell_id").is_some());
        assert!(cell.get("source").is_some());
        assert!(
            cell.get("status").is_none(),
            "status should be projected out"
        );
        assert!(
            cell.get("fixtures").is_none(),
            "fixtures should be projected out"
        );
    }
}

#[test]
fn view_cell_id_always_present_even_when_not_in_fields() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &["--fields", "source"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    for cell in &arr {
        assert!(
            cell.get("cell_id").is_some(),
            "cell_id must always be present"
        );
    }
}

#[test]
fn view_compute_total_has_fixtures_and_test() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 1);
    let cell = &arr[0];
    // fixtures present
    assert!(
        cell["fixtures"].is_object(),
        "compute-total should have fixtures"
    );
    // test present
    assert!(cell["test"].is_object(), "compute-total should have a test");
    // shas NOT exposed
    assert!(cell.get("shas").is_none(), "shas must not appear in output");
}

#[test]
fn view_status_field_structure() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    let status = &arr[0]["status"];
    assert!(status.get("valid").is_some(), "status.valid missing");
    assert!(
        status.get("diagnostics").is_some(),
        "status.diagnostics missing"
    );
    assert!(
        status["diagnostics"].is_array(),
        "diagnostics must be array"
    );
}

#[test]
fn view_stdin_mode() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let content = fs::read_to_string(&source_path).expect("read fixture");
    // Run with --stdin, piping notebook content
    let out = std::process::Command::new(common::binary())
        .args(["view", source_path.to_str().unwrap(), "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    let mut child = out;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 4, "stdin mode should return same 4 code cells");
}

#[test]
fn view_no_filters_returns_empty_array_for_no_code_cells() {
    // Build a notebook with only a markdown cell and verify output is []
    let dir = TempDir::new().expect("create tempdir");
    let nb_path = dir.path().join("md_only.ipynb");
    let nb_json = serde_json::json!({
        "cells": [
            {
                "cell_type": "markdown",
                "id": "md-only",
                "metadata": {},
                "source": ["# Hello"]
            }
        ],
        "metadata": {
            "kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
            "language_info": {"name": "python"}
        },
        "nbformat": 4,
        "nbformat_minor": 5
    });
    fs::write(&nb_path, nb_json.to_string()).expect("write notebook");
    let (stdout, _stderr, status) = run_view(&nb_path, &[]);
    assert!(status.success());
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 0);
}
