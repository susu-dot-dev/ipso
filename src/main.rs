mod diagnostics;
mod diff_utils;
mod edit;
mod filter;
mod mcp;
mod metadata;
mod notebook;
mod save;
mod shas;
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
        ///                                 (missing_sha, stale, diff_conflict,
        ///                                  missing_field, invalid_value,
        ///                                  unknown_cell)
        ///   diagnostics.severity:<level>  Has a diagnostic of this severity
        ///                                 (error, warning)
        ///
        /// Examples:
        ///   --filter "cell:compute-total"
        ///   --filter "index:2..4"
        ///   --filter "diagnostics.type:stale,diff_conflict"
        ///   --filter "status.valid:false" --filter "test:null"
        #[arg(long = "filter", verbatim_doc_comment)]
        filters: Vec<String>,
        /// Comma-separated list of fields to include in each cell object.
        /// `cell_id` is always included.  Default: all fields.
        #[arg(long)]
        fields: Option<String>,
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
        load_notebook(&path)
            .with_context(|| format!("loading notebook {}", path.display()))?
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
