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

/// Run `ipso edit <path>` (non-blocking — creates the editor notebook and
/// exits immediately). Returns the exit status.
fn run_edit(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args(["edit", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn ipso edit")
}

/// Run `ipso edit --continue <path>`. Returns the exit status.
fn run_edit_continue(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args(["edit", "--continue", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn ipso edit --continue")
}

/// Run `ipso edit --continue --force <path>`. Returns the exit status.
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
        .expect("spawn ipso edit --continue --force")
}

/// Run `ipso edit --clean <path>`. Returns the exit status.
fn run_edit_clean(source_path: &Path) -> std::process::ExitStatus {
    std::process::Command::new(common::binary())
        .args(["edit", "--clean", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn ipso edit --clean")
}

/// Run the full edit → modify → continue workflow:
///   1. `ipso edit <path>` — creates editor notebook and exits.
///   2. Call `modify` with the editor notebook path.
///   3. `ipso edit --continue <path>` — applies changes.
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
        .join(format!("{}.ipso.ipynb", stem));

    let edit_status = run_edit(source_path);
    assert!(
        edit_status.success(),
        "ipso edit exited non-zero during setup"
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

/// Return true if any cell in the notebook JSON has `ipso.editor` metadata.
fn has_editor_metadata(nb: &serde_json::Value) -> bool {
    nb["cells"].as_array().map_or(false, |cells| {
        cells.iter().any(|cell| {
            cell["metadata"]
                .get("ipso")
                .map_or(false, |ipso_meta| ipso_meta.get("editor").is_some())
        })
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full smoke test:
///   1. Run `ipso edit` on simple.ipynb — exits immediately.
///   2. Copy the editor notebook so we can execute it.
///   3. Run `ipso edit --continue` — applies changes.
///   4. Execute the copy via nbclient.
///   5. Validate cell outputs.
///   6. Validate the source notebook was saved back without editor metadata.
///   7. Validate the editor notebook was deleted after a successful apply.
#[test]
fn smoke_edit_executes_and_saves_cleanly() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.ipso.ipynb");
    let copy_path = dir.path().join("editor_copy.ipynb");
    let output_path = dir.path().join("output.ipynb");

    // --- edit step ---
    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "ipso edit exited non-zero");
    assert!(editor_path.exists(), "editor notebook not created");

    // Copy the editor notebook before --continue deletes it.
    fs::copy(&editor_path, &copy_path).expect("copy editor notebook for execution");

    // --- continue step ---
    let continue_status = run_edit_continue(&source_path);
    assert!(
        continue_status.success(),
        "ipso edit --continue exited non-zero"
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
        "expected '%%ipso_skip' to produce skip output but got:\n{all_output}"
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
        "source notebook still contains ipso.editor metadata after save"
    );
}

/// Attempting to edit when the editor file already exists must fail immediately
/// with a message suggesting --continue or --clean, and leave the source notebook untouched.
#[test]
fn edit_fails_if_editor_file_already_exists() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.ipso.ipynb");

    // Pre-create the editor file.
    fs::write(&editor_path, b"{}").expect("create dummy editor file");

    let source_before = fs::read(&source_path).expect("read source before");

    let output = std::process::Command::new(common::binary())
        .args(["edit", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .expect("spawn ipso");

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
    let editor_path = dir.path().join("simple.ipso.ipynb");

    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "ipso edit exited non-zero");
    assert!(editor_path.exists(), "editor notebook not created");

    let clean_status = run_edit_clean(&source_path);
    assert!(clean_status.success(), "ipso edit --clean exited non-zero");
    assert!(
        editor_path.exists(),
        "editor notebook does not exist after --clean (should be recreated)"
    );
}

/// `edit --clean` when no editor file exists succeeds silently and still creates a fresh one.
#[test]
fn edit_clean_fails_if_no_editor_file() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.ipso.ipynb");

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

/// Helper: extract the `ipso` metadata object from a cell identified by
/// its `id` field. Returns `serde_json::Value::Null` if not found.
fn cell_nb_meta<'a>(nb: &'a serde_json::Value, cell_id: &str) -> &'a serde_json::Value {
    if let Some(cells) = nb["cells"].as_array() {
        for cell in cells {
            if cell["id"].as_str() == Some(cell_id) {
                let meta = &cell["metadata"]["ipso"];
                if !meta.is_null() {
                    return meta;
                }
            }
        }
    }
    &serde_json::Value::Null
}

/// After a round-trip with no user modifications the `ipso` metadata on
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
    assert!(status.success(), "ipso edit --continue exited non-zero");

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
            "ipso metadata changed for cell '{cell_id}' despite no user edits \
             (ignoring newly-stamped shas)"
        );
    }
    assert!(
        !dir.path().join("simple.ipso.ipynb").exists(),
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
                let role = cell["metadata"]["ipso"]["editor"]["role"]
                    .as_str()
                    .unwrap_or("");
                let cell_id = cell["metadata"]["ipso"]["editor"]["cell_id"]
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
    assert!(status.success(), "ipso edit --continue exited non-zero");

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
                let role = cell["metadata"]["ipso"]["editor"]["role"]
                    .as_str()
                    .unwrap_or("");
                let cell_id = cell["metadata"]["ipso"]["editor"]["cell_id"]
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
    assert!(status.success(), "ipso edit --continue exited non-zero");

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
                c["metadata"]["ipso"]["editor"]["role"].as_str() == Some("patched-source")
                    && c["metadata"]["ipso"]["editor"]["cell_id"].as_str() == Some("compute-total")
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
    assert!(status.success(), "ipso edit --continue exited non-zero");

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
/// The ipso key itself is preserved (since the cell already had it).
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
    assert!(status.success(), "ipso edit --continue exited non-zero");

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    let meta = cell_nb_meta(&saved, "reviewed-pass");

    // The ipso key itself must still exist (cell already had it).
    assert!(!meta.is_null(), "ipso key was removed from 'reviewed-pass'");

    // In the new model, null fields are absent (not written as null).
    for key in &["fixtures", "diff", "test"] {
        assert!(
            meta.get(key).is_none(),
            "key '{key}' should be absent in 'reviewed-pass' after round-trip, got: {}",
            meta[key]
        );
    }
}

/// The section header for a cell with ipso but no shas must contain
/// "Needs review". `compute-total` in simple.ipynb has ipso metadata
/// but no `shas` entry → Missing state.
#[test]
fn edit_notebook_contains_guide_cells() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.ipso.ipynb");

    assert!(
        run_edit(&source_path).success(),
        "ipso edit exited non-zero"
    );

    let raw = fs::read_to_string(&editor_path).expect("read editor notebook");
    let nb: serde_json::Value = serde_json::from_str(&raw).expect("parse editor notebook");

    let guides = nb["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .filter(|c| c["metadata"]["ipso"]["editor"]["role"].as_str() == Some("guide"))
        .count();

    assert!(
        guides >= 8,
        "expected at least 8 guide markdown cells in simple.ipynb editor, got {guides}"
    );
}

#[test]
fn edit_continue_does_not_warn_on_guide_markdown_cells() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    assert!(
        run_edit(&source_path).success(),
        "ipso edit exited non-zero"
    );

    let output = std::process::Command::new(common::binary())
        .args(["edit", "--continue", source_path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn ipso edit --continue");

    assert!(
        output.status.success(),
        "edit --continue failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("non-code cell"),
        "official guide cells must not trigger ignore warning; stderr:\n{stderr}"
    );
}

#[test]
fn edit_unaccepted_cell_header_contains_needs_review_and_reason() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let editor_path = dir.path().join("simple.ipso.ipynb");

    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "ipso edit exited non-zero");

    let raw = fs::read_to_string(&editor_path).expect("read editor notebook");
    let nb: serde_json::Value = serde_json::from_str(&raw).expect("parse editor notebook");

    // Find the section-header cell for "compute-total".
    let header_src = nb["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .find(|c| {
            c["metadata"]["ipso"]["editor"]["role"].as_str() == Some("section-header")
                && c["metadata"]["ipso"]["editor"]["cell_id"].as_str() == Some("compute-total")
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
        header_src.to_lowercase().contains("sha")
            || header_src.to_lowercase().contains("accepted")
            || header_src.to_lowercase().contains("never"),
        "expected reason about missing shas in section header but got:\n{header_src}"
    );
}

/// After `edit --continue`, the test source stored in the notebook must not
/// begin with `%%ipso_skip` — it should have been stripped during apply.
#[test]
fn edit_continue_strips_ipso_skip_from_test_source() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(status.success(), "ipso edit --continue exited non-zero");

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    // Check that no cell's test.source starts with %%ipso_skip.
    if let Some(cells) = saved["cells"].as_array() {
        for cell in cells {
            let test_src = &cell["metadata"]["ipso"]["test"]["source"];
            if let Some(s) = test_src.as_str() {
                assert!(
                    !s.starts_with("%%ipso_skip"),
                    "test source still contains %%ipso_skip after --continue: {s}"
                );
            } else if let Some(arr) = test_src.as_array() {
                let joined = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                assert!(
                    !joined.starts_with("%%ipso_skip"),
                    "test source still contains %%ipso_skip after --continue: {joined}"
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
    assert!(edit_status.success(), "ipso edit exited non-zero");

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
/// stripping all ipso metadata and applying the editor notebook.
#[test]
fn edit_continue_force_succeeds_despite_conflict() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let edit_status = run_edit(&source_path);
    assert!(edit_status.success(), "ipso edit exited non-zero");

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
/// ipso metadata must have a `shas` snapshot stamped on them.
/// `compute-total` and `reviewed-pass` both have ipso in simple.ipynb.
#[test]
fn edit_continue_stamps_shas_on_ipso_cells() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(status.success(), "ipso edit --continue exited non-zero");

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    for cell_id in &["compute-total", "reviewed-pass"] {
        let meta = cell_nb_meta(&saved, cell_id);
        assert!(!meta.is_null(), "cell '{cell_id}' has no ipso metadata");
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

/// After `edit --continue`, cells without ipso metadata must NOT have
/// shas stamped on them — only cells that actually went through the editor
/// with ipso are touched. `plain-data` has no ipso in simple.ipynb.
#[test]
fn edit_continue_does_not_stamp_shas_on_plain_cells() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let _ = dir;

    let status = run_edit_with_modifications(&source_path, |_| {});
    assert!(status.success(), "ipso edit --continue exited non-zero");

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).expect("read saved"))
            .expect("parse saved");

    // plain-data has no ipso metadata — its ipso key must remain absent.
    let meta = cell_nb_meta(&saved, "plain-data");
    assert!(
        meta.is_null(),
        "plain-data should have no ipso metadata after --continue, got: {meta}"
    );
}

// ===========================================================================
// nb view tests
// ===========================================================================

/// Run `ipso view <args>` and return (stdout, stderr, exit status).
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
        .expect("spawn ipso view");
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

// ===========================================================================
// nb scaffold tests
// ===========================================================================

/// Run `ipso scaffold <args>` and return (stdout, stderr, exit status).
fn run_scaffold(args: &[&str]) -> (String, String, std::process::ExitStatus) {
    let mut cmd = std::process::Command::new(common::binary());
    cmd.arg("scaffold");
    for a in args {
        cmd.arg(a);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn ipso scaffold");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status)
}

#[test]
fn scaffold_fixture_produces_valid_json() {
    let (stdout, _stderr, status) = run_scaffold(&[
        "fixture",
        "--name",
        "setup_df",
        "--description",
        "Small test dataframe",
        "--priority",
        "1",
        "--source",
        "global df; df = pd.DataFrame({'amount': [1, 2, 3]})",
    ]);
    assert!(status.success());
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(val["fixtures"]["setup_df"].is_object());
    assert_eq!(
        val["fixtures"]["setup_df"]["description"].as_str(),
        Some("Small test dataframe")
    );
    assert_eq!(val["fixtures"]["setup_df"]["priority"].as_i64(), Some(1));
    assert!(val["fixtures"]["setup_df"]["source"].is_string());
}

#[test]
fn scaffold_fixture_minimal_uses_defaults() {
    let (stdout, _stderr, status) = run_scaffold(&["fixture", "--name", "my_fix"]);
    assert!(status.success());
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let fix = &val["fixtures"]["my_fix"];
    assert_eq!(fix["description"].as_str(), Some(""));
    assert_eq!(fix["priority"].as_i64(), Some(0));
    assert_eq!(fix["source"].as_str(), Some(""));
}

#[test]
fn scaffold_test_produces_valid_json() {
    let (stdout, _stderr, status) = run_scaffold(&[
        "test",
        "--name",
        "test_total",
        "--source",
        "assert total == 6",
    ]);
    assert!(status.success());
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(val["test"]["name"].as_str(), Some("test_total"));
    assert_eq!(val["test"]["source"].as_str(), Some("assert total == 6"));
}

#[test]
fn scaffold_test_minimal_uses_defaults() {
    let (stdout, _stderr, status) = run_scaffold(&["test", "--name", "test_foo"]);
    assert!(status.success());
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(val["test"]["name"].as_str(), Some("test_foo"));
    assert_eq!(val["test"]["source"].as_str(), Some(""));
}

// ===========================================================================
// nb status tests
// ===========================================================================

/// Run `ipso status <args>` and return (stdout, stderr, exit status).
fn run_status(
    source_path: &Path,
    extra_args: &[&str],
) -> (String, String, std::process::ExitStatus) {
    let mut cmd = std::process::Command::new(common::binary());
    cmd.arg("status").arg(source_path);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn ipso status");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status)
}

#[test]
fn status_exits_nonzero_when_invalid_cells_exist() {
    // simple.ipynb has cells with ipso but no shas → missing → invalid
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, status) = run_status(&source_path, &[]);
    assert!(
        !status.success(),
        "expected non-zero exit for invalid cells"
    );
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(!arr.is_empty(), "expected at least one invalid cell");
    // Each cell should only have cell_id and status
    for cell in &arr {
        assert!(cell.get("cell_id").is_some());
        assert!(cell.get("status").is_some());
        assert!(
            cell.get("source").is_none(),
            "status should only show cell_id,status"
        );
    }
}

#[test]
fn status_exits_zero_when_all_cells_valid() {
    // Accept all cells first, then check status
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // Accept all
    let accept_out = std::process::Command::new(common::binary())
        .args(["accept", source_path.to_str().unwrap(), "--all"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("spawn accept");
    assert!(accept_out.success());
    let (stdout, _stderr, status) = run_status(&source_path, &[]);
    assert!(
        status.success(),
        "expected zero exit when all cells are valid"
    );
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(arr.is_empty());
}

#[test]
fn status_with_filter_narrows_cells() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, _status) = run_status(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    // Should only contain compute-total if it's invalid
    for cell in &arr {
        assert_eq!(cell["cell_id"].as_str(), Some("compute-total"));
    }
}

// ===========================================================================
// nb accept tests
// ===========================================================================

/// Run `ipso accept <args>` and return (stdout, stderr, exit status).
fn run_accept(
    source_path: &Path,
    extra_args: &[&str],
) -> (String, String, std::process::ExitStatus) {
    let mut cmd = std::process::Command::new(common::binary());
    cmd.arg("accept").arg(source_path);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn ipso accept");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status)
}

#[test]
fn accept_requires_all_or_filter() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (_stdout, _stderr, status) = run_accept(&source_path, &[]);
    assert!(
        !status.success(),
        "accept without --all or --filter should fail"
    );
}

#[test]
fn accept_all_and_filter_together_rejected() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (_stdout, _stderr, status) =
        run_accept(&source_path, &["--all", "--filter", "cell:compute-total"]);
    assert!(
        !status.success(),
        "--all and --filter together should be rejected"
    );
}

#[test]
fn accept_all_makes_cells_valid() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // First confirm cells are invalid
    let (_stdout, _stderr, status_before) = run_status(&source_path, &[]);
    assert!(
        !status_before.success(),
        "cells should be invalid before accept"
    );

    // Accept all
    let (_stdout, _stderr, accept_status) = run_accept(&source_path, &["--all"]);
    assert!(accept_status.success());

    // Now status should pass
    let (_stdout, _stderr, status_after) = run_status(&source_path, &[]);
    assert!(
        status_after.success(),
        "cells should be valid after accept --all"
    );
}

#[test]
fn accept_with_filter_only_accepts_matching_cells() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // Accept only compute-total
    let (_stdout, _stderr, accept_status) =
        run_accept(&source_path, &["--filter", "cell:compute-total"]);
    assert!(accept_status.success());

    // compute-total should be valid now
    let (stdout, _stderr, _) = run_view(
        &source_path,
        &[
            "--filter",
            "cell:compute-total",
            "--fields",
            "cell_id,status",
        ],
    );
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["status"]["valid"], true);

    // reviewed-pass should still be invalid (has nb meta but no shas)
    let (stdout2, _stderr, _) = run_view(
        &source_path,
        &[
            "--filter",
            "cell:reviewed-pass",
            "--fields",
            "cell_id,status",
        ],
    );
    let arr2: Vec<serde_json::Value> = serde_json::from_str(&stdout2).expect("valid JSON");
    assert_eq!(arr2.len(), 1);
    assert_eq!(arr2[0]["status"]["valid"], false);
}

// ===========================================================================
// nb update tests
// ===========================================================================

/// Run `ipso update <args>` and return (stdout, stderr, exit status).
fn run_update(
    source_path: &Path,
    extra_args: &[&str],
) -> (String, String, std::process::ExitStatus) {
    let mut cmd = std::process::Command::new(common::binary());
    cmd.arg("update").arg(source_path);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn ipso update");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status)
}

#[test]
fn update_set_test_on_cell() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({
        "cell_id": "plain-data",
        "test": {
            "name": "test_data",
            "source": "assert data == [1, 2, 3]"
        }
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    // Verify test was written
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:plain-data"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr[0]["test"]["name"].as_str(), Some("test_data"));
}

#[test]
fn update_clear_test_with_null() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // compute-total has a test; clear it
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "test": null,
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        arr[0]["test"].is_null(),
        "test should be null after clearing"
    );
}

#[test]
fn update_merge_fixtures() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // compute-total already has setup_data fixture; add another
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "fixtures": {
            "new_fixture": {
                "description": "new one",
                "priority": 2,
                "source": "x = 42"
            }
        }
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    // Both old and new fixtures should be present
    assert!(
        arr[0]["fixtures"]["setup_data"].is_object(),
        "old fixture should be preserved"
    );
    assert!(
        arr[0]["fixtures"]["new_fixture"].is_object(),
        "new fixture should be added"
    );
}

#[test]
fn update_remove_specific_fixture() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "fixtures": {
            "setup_data": null
        }
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        arr[0]["fixtures"].is_null(),
        "fixtures should be null after removing only fixture"
    );
}

#[test]
fn update_clear_all_fixtures_with_null() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "fixtures": null,
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(arr[0]["fixtures"].is_null(), "fixtures should be null");
}

#[test]
fn update_unknown_cell_fails() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({
        "cell_id": "nonexistent-cell",
        "test": null,
    });
    let (_stdout, stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(!status.success());
    let diag: serde_json::Value = serde_json::from_str(&stderr).expect("valid JSON diagnostics");
    assert_eq!(diag["valid"], false);
    assert!(diag["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| { d["type"].as_str() == Some("invalid_field") }));
}

#[test]
fn update_invalid_fixture_missing_fields_fails() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "fixtures": {
            "bad_fixture": {
                "description": "missing priority and source"
            }
        }
    });
    let (_stdout, stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(!status.success());
    let diag: serde_json::Value = serde_json::from_str(&stderr).expect("valid JSON diagnostics");
    assert_eq!(diag["valid"], false);
    let types: Vec<&str> = diag["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["type"].as_str())
        .collect();
    assert!(
        types.contains(&"invalid_field"),
        "should report invalid_field diagnostics"
    );
}

#[test]
fn update_batch_multiple_cells() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!([
        {
            "cell_id": "plain-data",
            "test": {
                "name": "test_plain",
                "source": "assert True"
            }
        },
        {
            "cell_id": "compute-total",
            "diff": "some diff text"
        }
    ]);
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &[]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    let plain = arr
        .iter()
        .find(|c| c["cell_id"].as_str() == Some("plain-data"))
        .unwrap();
    assert_eq!(plain["test"]["name"].as_str(), Some("test_plain"));
    let compute = arr
        .iter()
        .find(|c| c["cell_id"].as_str() == Some("compute-total"))
        .unwrap();
    assert_eq!(compute["diff"].as_str(), Some("some diff text"));
}

#[test]
fn update_requires_data_or_data_file() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (_stdout, _stderr, status) = run_update(&source_path, &[]);
    assert!(
        !status.success(),
        "update without --data or --data-file should fail"
    );
}

#[test]
fn update_data_file_works() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let data_path = dir.path().join("updates.json");
    let data = serde_json::json!({
        "cell_id": "plain-data",
        "diff": "a diff string"
    });
    fs::write(&data_path, data.to_string()).expect("write data file");

    let (_stdout, _stderr, status) =
        run_update(&source_path, &["--data-file", data_path.to_str().unwrap()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:plain-data"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(arr[0]["diff"].as_str(), Some("a diff string"));
}

// ===========================================================================
// Additional integration tests — coverage gaps
// ===========================================================================

// --- update: --stdin mode ---

#[test]
fn update_stdin_mode_writes_to_stdout() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let content = fs::read_to_string(&source_path).expect("read fixture");
    let data = serde_json::json!({
        "cell_id": "plain-data",
        "diff": "stdin diff"
    });
    let child = std::process::Command::new(common::binary())
        .args([
            "update",
            source_path.to_str().unwrap(),
            "--stdin",
            "--data",
            &data.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    let mut child = child;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    // Stdout should contain the modified notebook JSON
    let stdout = String::from_utf8_lossy(&output.stdout);
    let nb: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid notebook JSON");
    assert!(nb["cells"].is_array());
    // Original file should be unchanged
    let original = fs::read_to_string(&source_path).expect("read original");
    let orig_nb: serde_json::Value = serde_json::from_str(&original).expect("parse original");
    // plain-data in original should NOT have a diff
    let plain_orig = orig_nb["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str() == Some("plain-data"))
        .unwrap();
    assert!(
        plain_orig["metadata"].get("ipso").is_none()
            || plain_orig["metadata"]["ipso"].get("diff").is_none()
            || plain_orig["metadata"]["ipso"]["diff"].is_null(),
        "original file should not be modified in --stdin mode"
    );
}

// --- update: both --data and --data-file rejected ---

#[test]
fn update_both_data_and_data_file_rejected() {
    let (dir, source_path) = setup_fixture("simple.ipynb");
    let data_path = dir.path().join("data.json");
    fs::write(&data_path, r#"{"cell_id":"plain-data"}"#).unwrap();
    let (_stdout, _stderr, status) = run_update(
        &source_path,
        &[
            "--data",
            r#"{"cell_id":"plain-data"}"#,
            "--data-file",
            data_path.to_str().unwrap(),
        ],
    );
    assert!(
        !status.success(),
        "should reject both --data and --data-file"
    );
}

// --- update: absent field = no-op ---

#[test]
fn update_absent_field_preserves_existing() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // compute-total has a test; update only sets diff, should preserve test
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "diff": "new diff"
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        arr[0]["test"].is_object(),
        "existing test should be preserved when not in update"
    );
    assert_eq!(arr[0]["diff"].as_str(), Some("new diff"));
}

// --- update: validation does not mutate notebook ---

#[test]
fn update_validation_failure_does_not_modify_notebook() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let before = fs::read(&source_path).expect("read before");
    let data = serde_json::json!({
        "cell_id": "nonexistent",
        "test": null,
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(!status.success());
    let after = fs::read(&source_path).expect("read after");
    assert_eq!(
        before, after,
        "notebook should not be modified on validation failure"
    );
}

// --- update: source and status fields ignored ---

#[test]
fn update_ignores_source_and_status_fields() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    // These extra fields should not cause an error and should be ignored
    let data = serde_json::json!({
        "cell_id": "plain-data",
        "source": "should be ignored",
        "status": {"valid": true, "diagnostics": []},
        "diff": "real update"
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:plain-data"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    // Source should NOT have changed
    assert_ne!(arr[0]["source"].as_str(), Some("should be ignored"));
    assert_eq!(arr[0]["diff"].as_str(), Some("real update"));
}

// --- update: empty array is no-op ---

#[test]
fn update_empty_array_is_noop() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let before = fs::read(&source_path).expect("read before");
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", "[]"]);
    assert!(status.success());
    let after = fs::read(&source_path).expect("read after");
    assert_eq!(
        before, after,
        "empty update array should not modify notebook"
    );
}

// --- update: upsert existing fixture values ---

#[test]
fn update_upsert_existing_fixture() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({
        "cell_id": "compute-total",
        "fixtures": {
            "setup_data": {
                "description": "Updated description",
                "priority": 99,
                "source": "data = [100]"
            }
        }
    });
    let (_stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "cell:compute-total"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    let fix = &arr[0]["fixtures"]["setup_data"];
    assert_eq!(fix["description"].as_str(), Some("Updated description"));
    assert_eq!(fix["priority"].as_i64(), Some(99));
}

// --- status: --stdin mode ---

#[test]
fn status_stdin_mode() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let content = fs::read_to_string(&source_path).expect("read fixture");
    let child = std::process::Command::new(common::binary())
        .args(["status", source_path.to_str().unwrap(), "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    let mut child = child;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    // simple.ipynb has invalid cells → non-zero exit
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(!arr.is_empty());
}

// --- accept: --stdin mode ---

#[test]
fn accept_stdin_mode_writes_to_stdout() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let content = fs::read_to_string(&source_path).expect("read fixture");
    let child = std::process::Command::new(common::binary())
        .args(["accept", source_path.to_str().unwrap(), "--stdin", "--all"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    let mut child = child;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let nb: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid notebook JSON");
    assert!(nb["cells"].is_array());
    // Original file should be unchanged
    let after = fs::read_to_string(&source_path).expect("read after");
    let after_nb: serde_json::Value = serde_json::from_str(&after).unwrap();
    // The original should NOT have shas stamped (accept wrote to stdout, not file)
    let ct = after_nb["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str() == Some("compute-total"))
        .unwrap();
    assert!(
        ct["metadata"]["ipso"].get("shas").is_none() || ct["metadata"]["ipso"]["shas"].is_null(),
        "original file should not be modified in --stdin mode"
    );
}

// --- accept: plain cells untouched by --all ---

#[test]
fn accept_all_stamps_plain_cells_with_shas_only() {
    // accept --all now creates ipso shas on plain code cells so they become
    // valid. Fixtures, diff, and test remain absent — accept does not invent them.
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (_stdout, _stderr, status) = run_accept(&source_path, &["--all"]);
    assert!(status.success());

    let (stdout, _stderr, _) = run_view(
        &source_path,
        &["--filter", "cell:plain-data", "--fields", "cell_id,status"],
    );
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        arr[0]["status"]["valid"].as_bool() == Some(true),
        "plain-data should be valid after accept --all"
    );

    // Fixtures, diff, and test must remain absent — accept only sets shas.
    let (stdout2, _stderr2, _) = run_view(&source_path, &["--filter", "cell:plain-data"]);
    let arr2: Vec<serde_json::Value> = serde_json::from_str(&stdout2).expect("valid JSON");
    assert!(
        arr2[0]["fixtures"].is_null() && arr2[0]["test"].is_null() && arr2[0]["diff"].is_null(),
        "plain cell should have no fixtures, diff, or test after accept"
    );
}

// --- accept: with diagnostics.type filter ---

#[test]
fn accept_with_diagnostics_type_filter() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (_stdout, _stderr, status) =
        run_accept(&source_path, &["--filter", "diagnostics.type:missing"]);
    assert!(status.success());

    // Cells that had missing should now be valid
    let (stdout, _stderr, _) = run_view(&source_path, &["--filter", "diagnostics.type:missing"]);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        arr.is_empty(),
        "no cells should have missing after accepting them"
    );
}

// --- view: filter by diff ---

#[test]
fn view_filter_diff_null() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, status) = run_view(&source_path, &["--filter", "diff:null"]);
    assert!(status.success());
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    for cell in &arr {
        assert!(
            cell["diff"].is_null(),
            "all cells with diff:null should have null diff"
        );
    }
}

// --- view: filter by diagnostics.type ---

#[test]
fn view_filter_diagnostics_type() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, status) =
        run_view(&source_path, &["--filter", "diagnostics.type:missing"]);
    assert!(status.success());
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    // Only cells with ipso but no shas should match
    for cell in &arr {
        assert!(
            cell["status"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["type"].as_str() == Some("missing")),
            "each result should have a missing diagnostic"
        );
    }
}

// --- view: filter by diagnostics.severity ---

#[test]
fn view_filter_diagnostics_severity() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let (stdout, _stderr, status) =
        run_view(&source_path, &["--filter", "diagnostics.severity:warning"]);
    assert!(status.success());
    let arr: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON");
    for cell in &arr {
        assert!(
            cell["status"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["severity"].as_str() == Some("warning")),
            "each result should have a warning-level diagnostic"
        );
    }
}

// --- view: --stdin with nonexistent path as hint ---

#[test]
fn view_stdin_nonexistent_path_hint_works() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let content = fs::read_to_string(&source_path).expect("read fixture");
    let child = std::process::Command::new(common::binary())
        .args(["view", "/nonexistent/path.ipynb", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    let mut child = child;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "should succeed with --stdin even if path doesn't exist"
    );
}

// --- scaffold: missing --name fails ---

#[test]
fn scaffold_fixture_missing_name_fails() {
    let (_stdout, _stderr, status) = run_scaffold(&["fixture", "--description", "d"]);
    assert!(!status.success());
}

#[test]
fn scaffold_test_missing_name_fails() {
    let (_stdout, _stderr, status) = run_scaffold(&["test", "--source", "s"]);
    assert!(!status.success());
}

// --- scaffold: output does not include cell_id ---

#[test]
fn scaffold_output_has_no_cell_id() {
    let (stdout, _stderr, _) = run_scaffold(&["fixture", "--name", "f1"]);
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(
        val.get("cell_id").is_none(),
        "scaffold should not include cell_id"
    );
}

// --- diagnostics: all fields present ---

#[test]
fn update_diagnostics_have_all_fields() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({"cell_id": "nonexistent"});
    let (_stdout, stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(!status.success());
    let diag: serde_json::Value = serde_json::from_str(&stderr).expect("valid JSON");
    let d = &diag["diagnostics"][0];
    assert!(d.get("type").is_some(), "diagnostic should have 'type'");
    assert!(
        d.get("severity").is_some(),
        "diagnostic should have 'severity'"
    );
    assert!(
        d.get("message").is_some(),
        "diagnostic should have 'message'"
    );
    assert!(d.get("field").is_some(), "diagnostic should have 'field'");
}

// --- diagnostics: stdout empty on validation failure ---

#[test]
fn update_validation_failure_stdout_empty() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let data = serde_json::json!({"cell_id": "nonexistent"});
    let (stdout, _stderr, status) = run_update(&source_path, &["--data", &data.to_string()]);
    assert!(!status.success());
    assert!(
        stdout.trim().is_empty(),
        "stdout should be empty on validation failure"
    );
}

// ===========================================================================
// ipso test
// ===========================================================================

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run `ipso test <path> [args...]` using the test venv Python.
/// Returns (stdout, stderr, exit_status).
fn run_ipso_test(
    nb_path: &std::path::Path,
    args: &[&str],
) -> (String, String, std::process::ExitStatus) {
    let output = std::process::Command::new(common::binary())
        .arg("test")
        .arg(nb_path)
        .arg("--python")
        .arg(common::python())
        .args(args)
        .output()
        .expect("spawn ipso test");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status,
    )
}

/// Parse the stdout of `ipso test` as a JSON array of results.
fn parse_test_results(stdout: &str) -> Vec<serde_json::Value> {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("failed to parse test results JSON: {e}\nstdout: {stdout}"))
}

// ---------------------------------------------------------------------------
// Success cases
// ---------------------------------------------------------------------------

#[test]
fn test_pass_exits_zero() {
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert!(
        status.success(),
        "expected exit 0 for passing test, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        status.code()
    );
}

#[test]
fn test_pass_result_is_completed() {
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"].as_str(), Some("completed"));
    assert_eq!(results[0]["cell_id"].as_str(), Some("compute-total"));
    assert_eq!(results[0]["test_name"].as_str(), Some("total is 60"));
}

#[test]
fn test_pass_implicit_subtest_passed() {
    // No explicit subtest() calls → single implicit subtest using test name
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(
        results[0]["status"].as_str(),
        Some("completed"),
        "expected status=completed but got: {}",
        serde_json::to_string_pretty(&results[0]).unwrap()
    );
    let subtests = results[0]["subtests"].as_array().unwrap();
    assert_eq!(subtests.len(), 1);
    assert_eq!(subtests[0]["passed"].as_bool(), Some(true));
    assert!(subtests[0]["error"].is_null());
    assert!(subtests[0]["traceback"].is_null());
}

#[test]
fn test_pass_fixture_runs_before_cell() {
    // Fixture overrides data=[10,20,30] so total==60, not 6 from data=[1,2,3]
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, _stderr, status) = run_ipso_test(&nb, &[]);
    assert!(status.success());
    let results = parse_test_results(&stdout);
    assert_eq!(results[0]["subtests"][0]["passed"].as_bool(), Some(true));
}

#[test]
fn test_pass_with_diff_applied() {
    // Diff patches `x = "original"` → `x = path_val`; fixture sets path_val = "patched"
    let nb = common::fixtures_dir().join("test-with-diff.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert!(
        status.success(),
        "expected exit 0\nstdout: {stdout}\nstderr: {stderr}"
    );
    let results = parse_test_results(&stdout);
    assert_eq!(results[0]["status"].as_str(), Some("completed"));
    assert_eq!(results[0]["subtests"][0]["passed"].as_bool(), Some(true));
}

// ---------------------------------------------------------------------------
// Test failure cases (exit 1)
// ---------------------------------------------------------------------------

#[test]
fn test_fail_assertion_exits_one() {
    let nb = common::fixtures_dir().join("test-fail-assertion.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert_eq!(
        status.code(),
        Some(1),
        "expected exit 1 for failing test\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn test_fail_assertion_result_is_completed_not_error() {
    // A test assertion failure is "completed" with passed=false, not an infra "error"
    let nb = common::fixtures_dir().join("test-fail-assertion.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(results[0]["status"].as_str(), Some("completed"));
}

#[test]
fn test_fail_assertion_subtest_not_passed() {
    let nb = common::fixtures_dir().join("test-fail-assertion.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(
        results[0]["status"].as_str(),
        Some("completed"),
        "expected status=completed but got: {}",
        serde_json::to_string_pretty(&results[0]).unwrap()
    );
    let subtests = results[0]["subtests"].as_array().unwrap();
    assert_eq!(subtests.len(), 1);
    assert_eq!(subtests[0]["passed"].as_bool(), Some(false));
}

#[test]
fn test_fail_assertion_error_and_traceback_present() {
    let nb = common::fixtures_dir().join("test-fail-assertion.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    let sub = &results[0]["subtests"][0];
    assert!(
        sub["error"].as_str().is_some(),
        "error field must be a string on failure"
    );
    assert!(
        sub["traceback"].as_str().is_some(),
        "traceback field must be a string on failure"
    );
    assert!(
        sub["error"].as_str().unwrap().contains("expected 2"),
        "error should contain assertion message"
    );

    let tb = sub["traceback"].as_str().expect("traceback string");
    assert!(
        !tb.contains('\x1b'),
        "traceback should be sanitized (no ESC): {tb:?}"
    );
}

// ---------------------------------------------------------------------------
// Subtest cases
// ---------------------------------------------------------------------------

#[test]
fn test_subtests_partial_fail_exits_one() {
    let nb = common::fixtures_dir().join("test-subtests.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert_eq!(
        status.code(),
        Some(1),
        "expected exit 1\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn test_subtests_both_reported() {
    // A failing subtest must not prevent subsequent subtests from running/reporting
    let nb = common::fixtures_dir().join("test-subtests.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(
        results[0]["status"].as_str(),
        Some("completed"),
        "expected status=completed but got: {}",
        serde_json::to_string_pretty(&results[0]).unwrap()
    );
    let subtests = results[0]["subtests"].as_array().unwrap();
    assert_eq!(subtests.len(), 2, "both subtests must be reported");
}

#[test]
fn test_subtests_first_passes_second_fails() {
    let nb = common::fixtures_dir().join("test-subtests.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(
        results[0]["status"].as_str(),
        Some("completed"),
        "expected status=completed but got: {}",
        serde_json::to_string_pretty(&results[0]).unwrap()
    );
    let subtests = results[0]["subtests"].as_array().unwrap();
    assert_eq!(
        subtests[0]["passed"].as_bool(),
        Some(true),
        "first subtest should pass"
    );
    assert_eq!(
        subtests[1]["passed"].as_bool(),
        Some(false),
        "second subtest should fail"
    );
}

#[test]
fn test_subtests_names_preserved() {
    let nb = common::fixtures_dir().join("test-subtests.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(
        results[0]["status"].as_str(),
        Some("completed"),
        "expected status=completed but got: {}",
        serde_json::to_string_pretty(&results[0]).unwrap()
    );
    let subtests = results[0]["subtests"].as_array().unwrap();
    assert_eq!(subtests[0]["name"].as_str(), Some("correct result"));
    assert_eq!(subtests[1]["name"].as_str(), Some("wrong assertion"));
}

// ---------------------------------------------------------------------------
// Infrastructure failure cases (exit 2)
// ---------------------------------------------------------------------------

#[test]
fn test_fixture_error_exits_two() {
    let nb = common::fixtures_dir().join("test-fixture-error.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert_eq!(
        status.code(),
        Some(2),
        "expected exit 2 for fixture error\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn test_fixture_error_status_is_error_not_completed() {
    // A fixture failure is an infrastructure error — must not be reported as
    // "completed" even though allow_errors=True lets execution continue past it
    let nb = common::fixtures_dir().join("test-fixture-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(
        results[0]["status"].as_str(),
        Some("error"),
        "fixture error must yield status=error, not completed"
    );
}

#[test]
fn test_fixture_error_phase_is_fixture() {
    let nb = common::fixtures_dir().join("test-fixture-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(results[0]["error"]["phase"].as_str(), Some("fixture"));
    assert_eq!(
        results[0]["error"]["fixture_name"].as_str(),
        Some("bad_fixture")
    );
}

#[test]
fn test_fixture_error_detail_contains_exception() {
    let nb = common::fixtures_dir().join("test-fixture-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    let detail = results[0]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("RuntimeError") && detail.contains("fixture exploded"),
        "detail should mention the exception, got: {detail}"
    );
}

#[test]
fn test_source_cell_error_exits_two() {
    let nb = common::fixtures_dir().join("test-source-error.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert_eq!(
        status.code(),
        Some(2),
        "expected exit 2 for source cell error\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn test_source_cell_error_status_is_error() {
    let nb = common::fixtures_dir().join("test-source-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(results[0]["status"].as_str(), Some("error"));
}

#[test]
fn test_source_cell_error_phase_is_cell_source() {
    let nb = common::fixtures_dir().join("test-source-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    assert_eq!(results[0]["error"]["phase"].as_str(), Some("cell_source"));
    assert_eq!(
        results[0]["error"]["source_cell_id"].as_str(),
        Some("bad-source")
    );
}

#[test]
fn test_source_cell_error_detail_contains_exception() {
    let nb = common::fixtures_dir().join("test-source-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    let detail = results[0]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("RuntimeError") && detail.contains("source cell exploded"),
        "detail should mention the exception, got: {detail}"
    );
}

// ---------------------------------------------------------------------------
// Parallel execution
// ---------------------------------------------------------------------------

#[test]
fn test_parallel_all_cells_run() {
    let nb = common::fixtures_dir().join("test-multi-cell.ipynb");
    let (stdout, stderr, status) = run_ipso_test(&nb, &[]);
    assert!(
        status.success(),
        "expected exit 0\nstdout: {stdout}\nstderr: {stderr}"
    );
    let results = parse_test_results(&stdout);
    assert_eq!(results.len(), 2, "both cells should be tested");
}

#[test]
fn test_parallel_correct_cell_ids_present() {
    let nb = common::fixtures_dir().join("test-multi-cell.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    let cell_ids: Vec<&str> = results
        .iter()
        .filter_map(|r| r["cell_id"].as_str())
        .collect();
    assert!(cell_ids.contains(&"cell-a"), "cell-a missing from results");
    assert!(cell_ids.contains(&"cell-b"), "cell-b missing from results");
}

#[test]
fn test_parallel_both_pass() {
    let nb = common::fixtures_dir().join("test-multi-cell.ipynb");
    let (stdout, _stderr, status) = run_ipso_test(&nb, &[]);
    assert!(status.success());
    let results = parse_test_results(&stdout);
    for r in &results {
        assert_eq!(r["status"].as_str(), Some("completed"));
        assert_eq!(r["subtests"][0]["passed"].as_bool(), Some(true));
    }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

#[test]
fn test_filter_selects_single_cell() {
    let nb = common::fixtures_dir().join("test-multi-cell.ipynb");
    let (stdout, _stderr, status) = run_ipso_test(&nb, &["--filter", "cell:cell-a"]);
    assert!(status.success());
    let results = parse_test_results(&stdout);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["cell_id"].as_str(), Some("cell-a"));
}

#[test]
fn test_filter_excludes_other_cell() {
    let nb = common::fixtures_dir().join("test-multi-cell.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &["--filter", "cell:cell-b"]);
    let results = parse_test_results(&stdout);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["cell_id"].as_str(), Some("cell-b"));
}

#[test]
fn test_no_matching_filter_returns_empty_array() {
    let nb = common::fixtures_dir().join("test-multi-cell.ipynb");
    let (stdout, _stderr, status) = run_ipso_test(&nb, &["--filter", "cell:nonexistent"]);
    assert!(status.success());
    let results = parse_test_results(&stdout);
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// CLI argument validation
// ---------------------------------------------------------------------------

#[test]
fn test_no_filter_runs_all_cells() {
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, _stderr, status) = run_ipso_test(&nb, &[]);
    assert!(
        status.success(),
        "expected zero exit when no --filter given"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
    assert!(parsed.as_array().is_some_and(|a| !a.is_empty()));
}

// ---------------------------------------------------------------------------
// Output schema
// ---------------------------------------------------------------------------

#[test]
fn test_output_is_valid_json_array() {
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout}"));
    assert!(parsed.is_array(), "output must be a JSON array");
}

#[test]
fn test_completed_result_schema() {
    let nb = common::fixtures_dir().join("test-pass.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    let r = &results[0];
    assert!(r.get("cell_id").is_some(), "missing cell_id");
    assert!(r.get("test_name").is_some(), "missing test_name");
    assert!(r.get("status").is_some(), "missing status");
    assert!(r.get("subtests").is_some(), "missing subtests");
    let sub = &r["subtests"][0];
    assert!(sub.get("name").is_some(), "subtest missing name");
    assert!(sub.get("passed").is_some(), "subtest missing passed");
    assert!(sub.get("error").is_some(), "subtest missing error key");
    assert!(
        sub.get("traceback").is_some(),
        "subtest missing traceback key"
    );
}

#[test]
fn test_error_result_schema() {
    let nb = common::fixtures_dir().join("test-fixture-error.ipynb");
    let (stdout, _stderr, _status) = run_ipso_test(&nb, &[]);
    let results = parse_test_results(&stdout);
    let r = &results[0];
    assert!(r.get("cell_id").is_some(), "missing cell_id");
    assert!(r.get("test_name").is_some(), "missing test_name");
    assert!(r.get("status").is_some(), "missing status");
    assert!(r.get("error").is_some(), "missing error object");
    let err = &r["error"];
    assert!(err.get("phase").is_some(), "error missing phase");
    assert!(err.get("detail").is_some(), "error missing detail");
}

// ---------------------------------------------------------------------------
// ipso upgrade
// ---------------------------------------------------------------------------

fn run_upgrade(
    source_path: &Path,
    extra_args: &[&str],
) -> (String, String, std::process::ExitStatus) {
    let mut cmd = std::process::Command::new(common::binary());
    cmd.arg("upgrade").arg(source_path);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn ipso upgrade");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr, out.status)
}

fn run_upgrade_stdin(source_path: &Path) -> (String, String, std::process::ExitStatus) {
    let content = fs::read_to_string(source_path).expect("read fixture");
    let mut cmd = std::process::Command::new(common::binary());
    cmd.args(["upgrade", "--stdin", source_path.to_str().unwrap()]);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ipso upgrade --stdin");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stdout, stderr, output.status)
}

/// Return the nbformat_minor from a parsed notebook JSON value.
fn nbformat_minor(nb: &serde_json::Value) -> i64 {
    nb["nbformat_minor"].as_i64().unwrap_or(-1)
}

/// Return true if every cell in the notebook has a non-empty "id" field.
fn all_cells_have_id(nb: &serde_json::Value) -> bool {
    nb["cells"]
        .as_array()
        .map(|cells| {
            cells
                .iter()
                .all(|c| c["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false))
        })
        .unwrap_or(false)
}

#[test]
fn upgrade_errors_on_legacy_notebook_via_view() {
    // ipso view should reject a 4.4 notebook with a clear upgrade message.
    let (_dir, source_path) = setup_fixture("legacy-44.ipynb");
    let (stdout, stderr, status) = run_view(&source_path, &[]);
    assert!(
        !status.success(),
        "expected non-zero exit for legacy notebook"
    );
    assert!(stdout.is_empty(), "stdout should be empty on error");
    assert!(
        stderr.contains("not nbformat 4.5"),
        "stderr should mention nbformat 4.5, got: {stderr}"
    );
    assert!(
        stderr.contains("ipso upgrade"),
        "stderr should mention `ipso upgrade`, got: {stderr}"
    );
}

#[test]
fn upgrade_command_upgrades_legacy_notebook() {
    let (_dir, source_path) = setup_fixture("legacy-44.ipynb");

    let (_stdout, stderr, status) = run_upgrade(&source_path, &[]);
    assert!(status.success(), "upgrade should exit 0; stderr: {stderr}");
    assert!(
        stderr.contains("Upgraded"),
        "stderr should contain summary, got: {stderr}"
    );

    let upgraded: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_path).unwrap()).unwrap();
    assert_eq!(nbformat_minor(&upgraded), 5, "nbformat_minor should be 5");
    assert!(all_cells_have_id(&upgraded), "all cells should have an id");

    // Cells with _cell_guid should use it as their id.
    let cells = upgraded["cells"].as_array().unwrap();
    assert_eq!(
        cells[0]["id"].as_str().unwrap(),
        "aa111111-0000-0000-0000-000000000001"
    );
    assert_eq!(
        cells[1]["id"].as_str().unwrap(),
        "bb222222-0000-0000-0000-000000000002"
    );
    assert_eq!(
        cells[2]["id"].as_str().unwrap(),
        "cc333333-0000-0000-0000-000000000003"
    );
    // Cell without _cell_guid gets a generated (non-empty) id.
    assert!(!cells[3]["id"].as_str().unwrap_or("").is_empty());
}

#[test]
fn upgrade_command_is_idempotent() {
    let (_dir, source_path) = setup_fixture("legacy-44.ipynb");
    let (_, _, status) = run_upgrade(&source_path, &[]);
    assert!(status.success(), "first upgrade should succeed");

    let (_stdout, stderr, status) = run_upgrade(&source_path, &[]);
    assert!(status.success(), "second upgrade should exit 0");
    assert!(
        stderr.contains("already nbformat 4.5"),
        "second upgrade stderr should say already 4.5, got: {stderr}"
    );
}

#[test]
fn upgrade_stdin_outputs_upgraded_notebook() {
    let (_dir, source_path) = setup_fixture("legacy-44.ipynb");
    let original_content = fs::read_to_string(&source_path).unwrap();

    let (stdout, _stderr, status) = run_upgrade_stdin(&source_path);
    assert!(status.success(), "upgrade --stdin should exit 0");

    let upgraded: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert_eq!(
        nbformat_minor(&upgraded),
        5,
        "stdout notebook should be nbformat 4.5"
    );
    assert!(all_cells_have_id(&upgraded), "all cells should have ids");

    // Original file should be untouched.
    assert_eq!(
        fs::read_to_string(&source_path).unwrap(),
        original_content,
        "--stdin must not modify the file on disk"
    );
}

#[test]
fn upgrade_dry_run_leaves_file_unchanged() {
    let (_dir, source_path) = setup_fixture("legacy-44.ipynb");
    let original_content = fs::read_to_string(&source_path).unwrap();

    let (stdout, _stderr, status) = run_upgrade(&source_path, &["--dry-run"]);
    assert!(status.success(), "upgrade --dry-run should exit 0");

    let upgraded: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON for --dry-run");
    assert_eq!(
        nbformat_minor(&upgraded),
        5,
        "--dry-run stdout should show nbformat 4.5"
    );

    assert_eq!(
        fs::read_to_string(&source_path).unwrap(),
        original_content,
        "--dry-run must not modify the file on disk"
    );
}

#[test]
fn upgrade_already_v45_is_noop() {
    let (_dir, source_path) = setup_fixture("simple.ipynb");
    let original_content = fs::read_to_string(&source_path).unwrap();

    let (_stdout, stderr, status) = run_upgrade(&source_path, &[]);
    assert!(status.success(), "upgrade on 4.5 notebook should exit 0");
    assert!(
        stderr.contains("already nbformat 4.5"),
        "stderr should say already 4.5, got: {stderr}"
    );

    assert_eq!(
        fs::read_to_string(&source_path).unwrap(),
        original_content,
        "4.5 notebook file must not be modified"
    );
}
