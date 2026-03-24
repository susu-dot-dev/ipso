# Maturin Wheel Packaging for nota-bene

## Problem

The `nota-bene` Python library depends on the Rust CLI binary being available
on `$PATH`. Currently these are distributed independently: the Rust CLI must be
built from source or obtained as a separate binary, and the Python library is a
pure-Python wheel. This creates a fragile setup experience — users must manually
ensure the CLI is installed and accessible.

The `pytest-nota-bene` plugin shells out to the `nota-bene` binary to run
notebook tests, so the binary must also be available wherever the plugin is
installed.

## Solution

Bundle the Rust CLI binary into the `nota-bene` Python wheel using **maturin**
as the PEP 517 build backend. When a user runs `pip install nota-bene`, they
get both the Python in-kernel library *and* the compiled `nota-bene` CLI binary,
which maturin places in the environment's `bin/` (or `Scripts\` on Windows)
directory via the wheel's `data/scripts` scheme.

### Build backend: maturin

maturin is the industry-standard tool for building Python wheels that contain
Rust-compiled artifacts. Configuration:

```toml
# nota-bene/pyproject.toml
[build-system]
requires = ["maturin>=1.0,<2.0"]
build-backend = "maturin"

[tool.maturin]
manifest-path = "../Cargo.toml"
bindings = "bin"
python-source = "src"
module-name = "nota_bene"
python-packages = ["nota_bene"]
```

Key settings:
- `manifest-path = "../Cargo.toml"` — the Rust crate lives in the repo root,
  one level above the Python package directory.
- `bindings = "bin"` — tells maturin this is a standalone binary (not a Python
  extension module). The compiled binary is installed into `scripts/`.
- `python-source = "src"` — includes the Python source tree (`src/nota_bene/`)
  in the wheel alongside the binary.
- `module-name = "nota_bene"` — maps the Cargo package name (`nota-bene`) to
  the Python module name (`nota_bene`), since maturin defaults to deriving the
  module name from the Cargo package name which uses hyphens.
- `python-packages = ["nota_bene"]` — explicitly lists the Python packages to
  include in the wheel.

### Publish workflow: maturin-action

The existing `nota-bene-publish.yaml` is replaced with a **maturin-action**
based workflow that builds platform-specific wheels:

| Target               | Runner           |
|----------------------|------------------|
| Linux x86_64         | ubuntu-latest    |
| macOS x86_64         | macos-13         |
| macOS arm64 (Apple Silicon) | macos-14   |
| Windows x86_64       | windows-latest   |

An additional job builds the sdist. All artifacts are uploaded and published to
PyPI on version tags via trusted publishing.

### CI simplification

The `ci.yml` workflow previously:
1. Built the Rust binary in a `rust` job and uploaded it as an artifact.
2. Downloaded the artifact in the `test-pnb` job before running pytest plugin tests.

With the binary now bundled into the `nota-bene` wheel, the `test-pnb` job
installs `nota-bene` (which provides the binary) directly. The artifact-passing
dance is removed. The standalone `rust` job is kept for fast lint feedback.

### pytest-nota-bene dependency update

`pytest-nota-bene` adds `nota-bene` as a runtime dependency (it was previously
dev-only). This ensures `pip install pytest-nota-bene` transitively installs
the CLI binary.

### End-to-end test

A new `e2e-wheel` CI job validates the full packaging pipeline:
1. Build the wheel using `maturin build` in `nota-bene/`.
2. Create a fresh virtual environment.
3. Install the wheel into that environment.
4. Assert `nota-bene --help` runs successfully and outputs expected text.
5. Assert the `nota_bene` Python package is importable.

This test also exists as a local script (`tests/test_wheel_e2e.sh`) that
developers can run to verify packaging before pushing.

## Local development

- `maturin develop` (run from `nota-bene/`) compiles the Rust binary and
  installs both it and the Python source into the current virtualenv.
- `pip install -e nota-bene/` also works via PEP 660 (editable installs),
  which delegates to maturin under the hood.
- A Rust toolchain is required for local development.

## Dependency chain (after this change)

```
pytest-nota-bene
  └── depends on: nota-bene (Python wheel)
        ├── Python library: nota_bene (in-kernel API, nbclient, ipykernel)
        └── Binary: nota-bene CLI (Rust, installed to bin/)
                └── invokes: python -m nota_bene._executor
```
