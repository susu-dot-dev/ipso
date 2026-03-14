mod diff_utils;
mod edit;
mod mcp;
mod metadata;
mod notebook;
mod save;
mod shas;

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};

use notebook::{load_notebook, save_notebook, CellExt};

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
