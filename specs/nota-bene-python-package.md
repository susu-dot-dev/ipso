---
name: nota-bene Python package
overview: Scaffold the nota-bene Python package under nota-bene/ using uv for dependency management and the uv build backend, with CI to build the wheel, versioning support, ruff/mypy linters, pytest with coverage, and pre-commit.
todos: []
isProject: false
---

# nota-bene Python package

## Context and repo layout

The repo contains a Rust crate at the root (`Cargo.toml`, `src/`). Python packages live as top-level sibling directories. This spec covers the `nota-bene` Python package only — a pure Python in-kernel library with no runtime dependencies.

```
.                               # repo root
├── Cargo.toml
├── src/                        # Rust crate
├── Makefile                    # delegates to sub-packages via nb-* targets
├── nota-bene/                  # THIS SPEC
│   ├── .coveragerc
│   ├── .pre-commit-config.yaml
│   ├── .python-version
│   ├── Makefile
│   ├── pyproject.toml
│   ├── uv.lock
│   ├── src/nota_bene/
│   └── tests/
└── pytest-nota-bene/           # future, separate spec
```

---

## 1. Build system and project metadata

Use `uv_build` as the build backend (not hatchling). Pin with an upper bound, e.g. `uv_build>=0.10.9,<0.11.0`, per uv's versioning policy. The `src/` layout is the uv default and requires no `module-root` override.

- `requires-python = ">=3.12"`
- `version = "0.1.0"` (manual, plain string — no dynamic versioning plugin)
- No runtime dependencies
- Exclude `tests/**` from the built distributions via `[tool.uv.build-backend]`

---

## 2. Dependency groups

Two groups managed by uv, both active by default via `[tool.uv] default-groups`:

- **dev**: `pytest`, `pytest-cov`
- **lint**: `ruff`, `mypy`

---

## 3. Linters and type checking

All configuration lives in `pyproject.toml`.

**ruff**: line length 120, `.venv` excluded, `quote-style = "preserve"` for formatting.

**mypy**: strict mode. Include a `py.typed` marker file in the package so downstream consumers get type information.

---

## 4. Tests and coverage

pytest with `pytest-cov`. Coverage configured to:
- Measure branch coverage
- Report XML (for CI) and HTML
- Source: `nota_bene` package only (test files excluded via `.coveragerc`)
- Omit `__about__.py`

---

## 5. Makefile

`nota-bene/Makefile` targets (all use `uv run` where appropriate):

| Target | Purpose |
|---|---|
| `sync` | `uv sync` |
| `sync-locked` | `uv sync --locked` (used in CI) |
| `test` | run pytest with coverage |
| `lint` | ruff check |
| `format` | ruff format check (dry-run) |
| `fix` | ruff check --fix + ruff format in place |
| `typing` | mypy on src and tests |
| `all` | lint + format + typing |
| `build` | `uv build` |

The root `Makefile` gets `nb-*` proxy targets (e.g. `nb-test`, `nb-lint`) that delegate to `$(MAKE) -C nota-bene <target>`, updated in the root `help` target.

---

## 6. Versioning

- Version string lives in `pyproject.toml` and mirrored in `src/nota_bene/__about__.py` for runtime inspection.
- To release: bump both locations, commit, push a `vX.Y.Z` tag — the publish workflow fires on that tag.

---

## 7. Pre-commit

A single local hook in `.pre-commit-config.yaml` that runs `uv run ruff format` on Python files at commit time. No external repo pin needed — ruff is already in the `lint` dependency group.

Activate with `uv run pre-commit install` from `nota-bene/` after cloning.

---

## 8. CI workflows

### `nota-bene-ci.yaml`

Triggers: push to `main`, PRs targeting `main`.

Two parallel jobs:
- **test**: `make sync-locked` → `make test` → upload `coverage.xml` and `htmlcov/` as artifacts
- **lint**: `make sync-locked` → `make all`

A third job runs only on PRs: downloads the coverage artifact and posts a coverage summary comment on the PR.

### `nota-bene-publish.yaml`

Triggers: push to `main`, PRs to `main`, and `vX.Y.Z` tags.

- **build** (always): `make build`, upload `dist/` as artifact — verifies the wheel builds on every change.
- **publish** (tags only): downloads the artifact and publishes to PyPI using trusted publishing (OIDC, `id-token: write`). Requires a `pypi` GitHub environment configured in repo settings and a trusted publisher registered on PyPI. No API token needed.

Both workflows set `working-directory: nota-bene` and use `astral-sh/setup-uv` with caching enabled.

---

## 9. Implementation notes

- `uv init --lib` can bootstrap the directory structure but defaults to `hatchling`; replace `pyproject.toml` with the configuration above afterward.
- Commit `uv.lock` for reproducible CI installs.
- mypy strict mode requires type annotations throughout; the `py.typed` marker ensures the package is recognized as typed by downstream tools.
