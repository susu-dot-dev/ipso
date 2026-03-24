use std::path::PathBuf;

/// Path to the compiled `ipso` binary, set by Cargo at test-build time.
pub fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ipso"))
}

/// Path to the Python interpreter inside the test venv.
///
/// Panics with a helpful message if the venv hasn't been created yet.
pub fn python() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/.venv/bin/python");
    assert!(
        p.exists(),
        "test venv not found at {:?} — run `make test-setup` first",
        p
    );
    p
}

/// Path to `tests/execute_nb.py`.
pub fn execute_nb_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/execute_nb.py")
}

/// Path to the `tests/fixtures/` directory.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}
