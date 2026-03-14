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
/// every source cell must match the original values.
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

    for cell_id in &["plain-data", "compute-total", "reviewed-pass"] {
        let before = cell_nb_meta(&original, cell_id);
        let after = cell_nb_meta(&saved, cell_id);
        assert_eq!(
            before, after,
            "nota-bene metadata changed for cell '{cell_id}' despite no user edits"
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

/// A cell that was already marked as reviewed (explicit `null` values for
/// `fixtures`, `diff`, and `test`) keeps those explicit nulls after a
/// round-trip with no user modifications.
#[test]
fn edit_explicit_nulls_survive_round_trip() {
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

    for key in &["fixtures", "diff", "test"] {
        assert!(
            meta.get(key).is_some(),
            "key '{key}' was removed from 'reviewed-pass' metadata"
        );
        assert!(
            meta[key].is_null(),
            "key '{key}' in 'reviewed-pass' is no longer null: {}",
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
