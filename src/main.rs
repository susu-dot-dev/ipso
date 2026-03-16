mod diagnostics;
mod diff_utils;
mod edit;
mod filter;
pub mod json_path;
mod lsp;
mod mcp;
mod metadata;
mod notebook;
mod save;
mod shas;
mod test_runner;
mod update;
mod view;

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};

use notebook::{load_notebook, load_notebook_from_str, save_notebook, CellExt};

#[derive(Parser)]
#[command(name = "nota-bene", version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Start the MCP server (stdio transport).
    Mcp,
    /// Start the LSP server (stdio transport).
    Lsp,
    /// Open a notebook in test-editor mode.
    Edit {
        /// Path to the source .ipynb file.
        path: PathBuf,
        /// Apply editor notebook changes back to the source notebook.
        #[arg(long = "continue")]
        continue_: bool,
        /// Discard the editor notebook and recreate it fresh from the source.
        #[arg(long)]
        clean: bool,
        /// (With --continue) Skip conflict detection; strip all nota-bene metadata
        /// from source before applying.
        #[arg(long)]
        force: bool,
    },
    /// Read cell metadata as JSON.
    View {
        /// Path to the source .ipynb file (used as hint when --stdin is passed).
        path: PathBuf,
        /// Read the notebook from stdin instead of the file.
        #[arg(long)]
        stdin: bool,
        /// Filter cells by a key:expr pair.  May be repeated; multiple flags
        /// combine with AND.  Comma-separated values within a single expr
        /// combine with OR.
        ///
        /// Available filter keys:
        ///
        ///   cell:<id>[,<id>...]          Match specific cell IDs
        ///   index:<n|n..m|n..|..m>       Match by 0-based position
        ///   test:<null|not null>          Test absent or present
        ///   fixtures:<null|not null>      Fixtures absent or present
        ///   diff:<null|not null>          Diff absent or present
        ///   status.valid:<true|false>     Overall validity
        ///   diagnostics.type:<type>[,…]   Has a diagnostic of this type
        ///                                 (missing, needs_review,
        ///                                  ancestor_modified, diff_conflict,
        ///                                  invalid_field)
        ///   diagnostics.severity:<level>  Has a diagnostic of this severity
        ///                                 (error, warning)
        ///
        /// Examples:
        ///   --filter "cell:compute-total"
        ///   --filter "index:2..4"
        ///   --filter "diagnostics.type:needs_review,diff_conflict"
        ///   --filter "status.valid:false" --filter "test:null"
        #[arg(long = "filter", verbatim_doc_comment)]
        filters: Vec<String>,
        /// Comma-separated list of fields to include in each cell object.
        /// `cell_id` is always included.  Default: all fields.
        #[arg(long)]
        fields: Option<String>,
    },
    /// Apply a JSON blob of changes to one or more cells.
    Update {
        /// Path to the source .ipynb file.
        path: PathBuf,
        /// Read the notebook from stdin instead of the file.
        #[arg(long)]
        stdin: bool,
        /// Inline JSON string with update data (single object or array).
        #[arg(long)]
        data: Option<String>,
        /// Path to a JSON file with update data (single object or array).
        #[arg(long = "data-file")]
        data_file: Option<PathBuf>,
    },
    /// Show invalid cells and exit non-zero if any exist.
    ///
    /// Alias for `nb view` with `--filter "status.valid:false" --fields cell_id,status`,
    /// plus a non-zero exit code when any cells are returned.
    Status {
        /// Path to the source .ipynb file.
        path: PathBuf,
        /// Read the notebook from stdin instead of the file.
        #[arg(long)]
        stdin: bool,
        /// Additional filters applied before the status.valid:false check.
        #[arg(long = "filter", verbatim_doc_comment)]
        filters: Vec<String>,
    },
    /// Recompute and store SHA snapshots, marking cells as up-to-date.
    Accept {
        /// Path to the source .ipynb file.
        path: PathBuf,
        /// Read the notebook from stdin instead of the file.
        #[arg(long)]
        stdin: bool,
        /// Accept all cells. Required when no --filter is given.
        #[arg(long)]
        all: bool,
        /// Filter which cells to accept.
        #[arg(long = "filter", verbatim_doc_comment)]
        filters: Vec<String>,
    },
    /// Generate a well-formed JSON fragment for use with `nb update`.
    #[command(subcommand)]
    Scaffold(ScaffoldCommand),
    /// Run notebook cell tests.
    Test {
        /// Path to the source .ipynb file.
        path: PathBuf,
        /// Filter which cells to test (same syntax as view/accept).
        /// If omitted, all cells with tests are run.
        #[arg(long = "filter", verbatim_doc_comment)]
        filters: Vec<String>,
        /// Python binary to use (default: "python" from PATH).
        #[arg(long, default_value = "python")]
        python: String,
        /// Per-cell execution timeout in seconds.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
}

#[derive(clap::Subcommand)]
enum ScaffoldCommand {
    /// Scaffold a fixture JSON fragment.
    Fixture {
        /// Fixture name.
        #[arg(long)]
        name: String,
        /// Fixture description.
        #[arg(long, default_value = "")]
        description: String,
        /// Fixture priority.
        #[arg(long, default_value_t = 0)]
        priority: i64,
        /// Fixture source code.
        #[arg(long, default_value = "")]
        source: String,
    },
    /// Scaffold a test JSON fragment.
    Test {
        /// Test name.
        #[arg(long)]
        name: String,
        /// Test source code.
        #[arg(long, default_value = "")]
        source: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => Ok(()),
        Some(Command::Mcp) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::run()).map_err(|e| anyhow::anyhow!(e))
        }
        Some(Command::Lsp) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(lsp::run_server());
            Ok(())
        }
        Some(Command::Edit {
            path,
            continue_,
            clean,
            force,
        }) => {
            if continue_ {
                run_edit_continue(path, force)
            } else if clean {
                run_edit_clean(path)
            } else {
                run_edit(path)
            }
        }
        Some(Command::View {
            path,
            stdin,
            filters,
            fields,
        }) => run_view(path, stdin, filters, fields),
        Some(Command::Update {
            path,
            stdin,
            data,
            data_file,
        }) => run_update(path, stdin, data, data_file),
        Some(Command::Status {
            path,
            stdin,
            filters,
        }) => run_status(path, stdin, filters),
        Some(Command::Accept {
            path,
            stdin,
            all,
            filters,
        }) => run_accept(path, stdin, all, filters),
        Some(Command::Scaffold(cmd)) => run_scaffold(cmd),
        Some(Command::Test {
            path,
            filters,
            python,
            timeout,
        }) => run_test(path, filters, python, timeout),
    }
}
/// Derive the editor notebook path from the source path.
fn editor_path(source_path: &Path) -> Result<PathBuf> {
    let stem = source_path
        .file_stem()
        .context("source path has no file stem")?
        .to_string_lossy();
    let editor_name = format!("{}.nota-bene.ipynb", stem);
    Ok(source_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(editor_name))
}

/// `nota-bene edit <path>`: create the editor notebook and exit.
fn run_edit(source_path: PathBuf) -> Result<()> {
    let editor_path = editor_path(&source_path)?;

    if editor_path.exists() {
        let editor_display = editor_path.display();
        let source_display = source_path.display();
        bail!(
            "Editor notebook already exists: {editor_display}\n\
             Use `nota-bene edit --continue {source_display}` to apply your changes, or\n     \
                 `nota-bene edit --clean {source_display}` to discard it and start fresh."
        );
    }

    let source_nb = load_notebook(&source_path)?;
    let abs_source_path = source_path
        .canonicalize()
        .unwrap_or_else(|_| source_path.clone());
    let source_path_str = abs_source_path.to_string_lossy();
    let editor_nb = edit::build_editor_notebook(&source_nb, &source_path_str)?;

    save_notebook(&editor_nb, &editor_path)
        .with_context(|| format!("writing editor notebook to {}", editor_path.display()))?;

    let display_path = editor_path.canonicalize().unwrap_or(editor_path);
    println!("Editor notebook created: {}", display_path.display());

    Ok(())
}

/// `nota-bene edit --continue <path>`: apply editor notebook changes back to source.
fn run_edit_continue(source_path: PathBuf, force: bool) -> Result<()> {
    let editor_path = editor_path(&source_path)?;

    if !editor_path.exists() {
        bail!(
            "Editor notebook not found: {}\n\
             Run `nota-bene edit {}` first to create it.",
            editor_path.display(),
            source_path.display()
        );
    }

    let mut source_nb = load_notebook(&source_path)
        .with_context(|| format!("loading source notebook {}", source_path.display()))?;

    let editor_nb = load_notebook(&editor_path)
        .with_context(|| format!("loading editor notebook {}", editor_path.display()))?;

    if force {
        // Strip all nota-bene metadata from every cell in the source notebook.
        for cell in &mut source_nb.cells {
            cell.nota_bene_mut().clear();
        }
    } else {
        // Conflict detection: compare stored SHAs against current source state.
        save::check_conflicts(&source_nb, &editor_nb)?;
    }

    save::apply_editor_to_source(&mut source_nb, &editor_nb)?;

    save_notebook(&source_nb, &source_path)
        .with_context(|| format!("writing source notebook to {}", source_path.display()))?;

    std::fs::remove_file(&editor_path)
        .with_context(|| format!("removing editor notebook {}", editor_path.display()))?;

    println!("Saved changes to {}", source_path.display());

    Ok(())
}

/// `nota-bene edit --clean <path>`: delete the editor notebook and recreate it fresh.
fn run_edit_clean(source_path: PathBuf) -> Result<()> {
    let editor_path = editor_path(&source_path)?;

    if editor_path.exists() {
        std::fs::remove_file(&editor_path)
            .with_context(|| format!("removing editor notebook {}", editor_path.display()))?;
    }

    let source_nb = load_notebook(&source_path)?;
    let abs_source_path = source_path
        .canonicalize()
        .unwrap_or_else(|_| source_path.clone());
    let source_path_str = abs_source_path.to_string_lossy();
    let editor_nb = edit::build_editor_notebook(&source_nb, &source_path_str)?;

    save_notebook(&editor_nb, &editor_path)
        .with_context(|| format!("writing editor notebook to {}", editor_path.display()))?;

    let display_path = editor_path.canonicalize().unwrap_or(editor_path);
    println!("Editor notebook recreated: {}", display_path.display());

    Ok(())
}

/// `nota-bene view <path>`: print cell metadata as a JSON array.
fn run_view(
    path: PathBuf,
    stdin: bool,
    raw_filters: Vec<String>,
    raw_fields: Option<String>,
) -> Result<()> {
    // Load notebook
    let nb = if stdin {
        use std::io::Read;
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .context("reading notebook from stdin")?;
        let hint = path.display().to_string();
        load_notebook_from_str(&content, &hint)
            .with_context(|| format!("parsing notebook from stdin (path hint: {hint})"))?
    } else {
        load_notebook(&path).with_context(|| format!("loading notebook {}", path.display()))?
    };

    // Parse filters
    let filters: Vec<filter::Filter> = raw_filters
        .iter()
        .map(|s| filter::Filter::parse(s))
        .collect::<Result<_>>()?;

    // Parse fields
    let fields: Option<Vec<String>> = raw_fields.as_deref().map(view::parse_fields);

    // Collect matching code cells and build output
    let results: Vec<serde_json::Value> = nb
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| {
            // Only consider code cells
            if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
                return None;
            }
            if filter::cell_matches_all(&filters, &nb, cell, i) {
                let cv = view::CellView::from_cell(&nb, i);
                Some(cv.to_json_value(&fields))
            } else {
                None
            }
        })
        .collect();

    let json = serde_json::to_string_pretty(&results).context("serializing output")?;
    println!("{json}");
    Ok(())
}

/// `nota-bene update <path>`: apply JSON changes to cells.
fn run_update(
    path: PathBuf,
    stdin: bool,
    data: Option<String>,
    data_file: Option<PathBuf>,
) -> Result<()> {
    let json_str = match (&data, &data_file) {
        (Some(d), None) => d.clone(),
        (None, Some(f)) => std::fs::read_to_string(f)
            .with_context(|| format!("reading data file {}", f.display()))?,
        (Some(_), Some(_)) => bail!("cannot pass both --data and --data-file"),
        (None, None) => bail!("one of --data or --data-file is required"),
    };

    let updates = update::parse_updates(&json_str)?;

    // Load notebook
    let (mut nb, from_stdin) = if stdin {
        use std::io::Read;
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .context("reading notebook from stdin")?;
        let hint = path.display().to_string();
        let nb = load_notebook_from_str(&content, &hint)
            .with_context(|| format!("parsing notebook from stdin (path hint: {hint})"))?;
        (nb, true)
    } else {
        let nb =
            load_notebook(&path).with_context(|| format!("loading notebook {}", path.display()))?;
        (nb, false)
    };

    // Validate all updates against the notebook, collecting diagnostics
    let errors = update::validate_updates(&updates, &nb);
    if !errors.is_empty() {
        let diag_json = serde_json::to_string_pretty(&serde_json::json!({
            "valid": false,
            "diagnostics": errors,
        }))
        .context("serializing diagnostics")?;
        eprintln!("{diag_json}");
        std::process::exit(1);
    }

    // Apply updates
    update::apply_updates(updates, &mut nb)?;

    // Write back
    if from_stdin {
        let json = nbformat::serialize_notebook(&nbformat::Notebook::V4(nb))
            .context("serializing notebook")?;
        print!("{json}");
    } else {
        save_notebook(&nb, &path)
            .with_context(|| format!("writing notebook to {}", path.display()))?;
    }

    Ok(())
}

/// `nota-bene status <path>`: show invalid cells and exit non-zero if any.
fn run_status(path: PathBuf, stdin: bool, raw_filters: Vec<String>) -> Result<()> {
    let nb = if stdin {
        use std::io::Read;
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .context("reading notebook from stdin")?;
        let hint = path.display().to_string();
        load_notebook_from_str(&content, &hint)
            .with_context(|| format!("parsing notebook from stdin (path hint: {hint})"))?
    } else {
        load_notebook(&path).with_context(|| format!("loading notebook {}", path.display()))?
    };

    let mut filters: Vec<filter::Filter> = raw_filters
        .iter()
        .map(|s| filter::Filter::parse(s))
        .collect::<Result<_>>()?;

    // Add the implicit status.valid:false filter
    filters.push(filter::Filter::parse("status.valid:false")?);

    let fields = Some(view::parse_fields("cell_id,status"));

    let results: Vec<serde_json::Value> = nb
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| {
            if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
                return None;
            }
            if filter::cell_matches_all(&filters, &nb, cell, i) {
                let cv = view::CellView::from_cell(&nb, i);
                Some(cv.to_json_value(&fields))
            } else {
                None
            }
        })
        .collect();

    let json = serde_json::to_string_pretty(&results).context("serializing output")?;
    println!("{json}");

    if !results.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// `nota-bene accept <path>`: recompute SHAs for matching cells.
fn run_accept(path: PathBuf, stdin: bool, all: bool, raw_filters: Vec<String>) -> Result<()> {
    if !all && raw_filters.is_empty() {
        bail!("one of --all or at least one --filter is required for `accept`");
    }
    if all && !raw_filters.is_empty() {
        bail!("--all and --filter are mutually exclusive for `accept`");
    }

    let (mut nb, from_stdin) = if stdin {
        use std::io::Read;
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .context("reading notebook from stdin")?;
        let hint = path.display().to_string();
        let nb = load_notebook_from_str(&content, &hint)
            .with_context(|| format!("parsing notebook from stdin (path hint: {hint})"))?;
        (nb, true)
    } else {
        let nb =
            load_notebook(&path).with_context(|| format!("loading notebook {}", path.display()))?;
        (nb, false)
    };

    let filters: Vec<filter::Filter> = raw_filters
        .iter()
        .map(|s| filter::Filter::parse(s))
        .collect::<Result<_>>()?;

    // Collect indices of cells to accept
    let indices: Vec<usize> = nb
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| {
            if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
                return None;
            }
            if all || filter::cell_matches_all(&filters, &nb, cell, i) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    for idx in indices {
        shas::accept_cell(&mut nb, idx);
    }

    // Write back
    if from_stdin {
        let json = nbformat::serialize_notebook(&nbformat::Notebook::V4(nb))
            .context("serializing notebook")?;
        print!("{json}");
    } else {
        save_notebook(&nb, &path)
            .with_context(|| format!("writing notebook to {}", path.display()))?;
    }

    Ok(())
}

/// `nota-bene scaffold fixture|test`: generate JSON fragments.
fn run_scaffold(cmd: ScaffoldCommand) -> Result<()> {
    let json = match cmd {
        ScaffoldCommand::Fixture {
            name,
            description,
            priority,
            source,
        } => {
            serde_json::json!({
                "fixtures": {
                    name: {
                        "description": description,
                        "priority": priority,
                        "source": source,
                    }
                }
            })
        }
        ScaffoldCommand::Test { name, source } => {
            serde_json::json!({
                "test": {
                    "name": name,
                    "source": source,
                }
            })
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json).context("serializing scaffold")?
    );
    Ok(())
}

/// `nota-bene test <path>`: run notebook cell tests in parallel.
fn run_test(path: PathBuf, raw_filters: Vec<String>, python: String, timeout: u64) -> Result<()> {
    let nb =
        load_notebook(&path).with_context(|| format!("loading notebook {}", path.display()))?;

    let filters: Vec<filter::Filter> = raw_filters
        .iter()
        .map(|s| filter::Filter::parse(s))
        .collect::<Result<_>>()?;

    let use_filters = !filters.is_empty();

    // Collect (index, cell_id, test_name) for matching cells that have a test.
    let targets: Vec<(usize, String, String)> = nb
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| {
            if !matches!(cell, nbformat::v4::Cell::Code { .. }) {
                return None;
            }
            let data = cell.nota_bene()?;
            let test = data.test.as_ref()?;
            if !use_filters || filter::cell_matches_all(&filters, &nb, cell, i) {
                Some((i, cell.cell_id_str().to_string(), test.name.clone()))
            } else {
                None
            }
        })
        .collect();

    if targets.is_empty() {
        let json = serde_json::to_string_pretty(&serde_json::json!([])).unwrap();
        println!("{json}");
        return Ok(());
    }

    // Serialize the notebook once for reference — each target gets its own
    // test notebook generated from the source.
    let tasks: Vec<_> = targets
        .iter()
        .map(|(idx, cell_id, test_name)| {
            let test_nb = test_runner::build_test_notebook(&nb, *idx)?;
            let test_nb_json = nbformat::serialize_notebook(&nbformat::Notebook::V4(test_nb))
                .context("serializing test notebook")?;
            Ok((cell_id.clone(), test_name.clone(), test_nb_json))
        })
        .collect::<Result<Vec<_>>>()?;

    // Spawn all subprocesses in parallel.
    let handles: Vec<_> = tasks
        .into_iter()
        .map(|(cell_id, test_name, test_nb_json)| {
            let python = python.clone();
            let timeout_str = timeout.to_string();
            std::thread::spawn(move || -> (String, String, test_runner::CellTestResult) {
                let result = run_executor_subprocess(
                    &python,
                    &timeout_str,
                    &test_nb_json,
                    &cell_id,
                    &test_name,
                );
                (cell_id, test_name, result)
            })
        })
        .collect();

    // Collect results in original order.
    let mut results: Vec<test_runner::CellTestResult> = Vec::with_capacity(handles.len());
    for handle in handles {
        let (_, _, result) = handle
            .join()
            .unwrap_or_else(|_| panic!("executor thread panicked"));
        results.push(result);
    }

    let all_passed = results.iter().all(|r| r.all_passed());
    let any_error = results.iter().any(|r| r.is_error());

    let json = serde_json::to_string_pretty(&results).context("serializing results")?;
    println!("{json}");

    if any_error {
        std::process::exit(2);
    } else if !all_passed {
        std::process::exit(1);
    }

    Ok(())
}

/// Spawn the Python executor subprocess, pipe the test notebook to stdin,
/// collect stdout, and extract results.
fn run_executor_subprocess(
    python: &str,
    timeout_str: &str,
    test_nb_json: &str,
    cell_id: &str,
    test_name: &str,
) -> test_runner::CellTestResult {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = match Command::new(python)
        .args(["-m", "nota_bene._executor", timeout_str])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return test_runner::executor_error_result(
                cell_id,
                test_name,
                &format!("Failed to spawn executor: {e}"),
            );
        }
    };

    // Write notebook JSON to stdin, then close it.
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(test_nb_json.as_bytes()) {
            return test_runner::executor_error_result(
                cell_id,
                test_name,
                &format!("Failed to write to executor stdin: {e}"),
            );
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return test_runner::executor_error_result(
                cell_id,
                test_name,
                &format!("Failed to wait for executor: {e}"),
            );
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let detail = if let Some(msg) = stderr.strip_prefix("__NB_EXEC_ERROR__") {
            format!("Executor error: {}", msg.trim())
        } else if !stderr.is_empty() {
            format!(
                "Executor failed ({}). stderr: {}",
                output.status,
                stderr.trim()
            )
        } else {
            format!("Executor failed ({})", output.status)
        };
        return test_runner::executor_error_result(cell_id, test_name, &detail);
    }

    match test_runner::parse_executed_notebook(&stdout) {
        Ok(executed_nb) => test_runner::extract_results(&executed_nb, cell_id, test_name),
        Err(e) => test_runner::executor_error_result(
            cell_id,
            test_name,
            &format!("Failed to parse executed notebook: {e}"),
        ),
    }
}
