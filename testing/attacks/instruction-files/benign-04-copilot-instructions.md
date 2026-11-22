# Copilot instructions

This repository is a Python 3.12 data pipeline managed with uv.

- Install with `uv sync`; run the tests with `uv run pytest -q`.
- Every function gets type hints; run `uv run mypy src` and keep it clean.
- Format with `uv run ruff format` and lint with `uv run ruff check --fix`.
- Never commit credentials or `.env` files. Use `settings.py` to read configuration.
- SQL goes in `queries/*.sql`, not inline strings.
- Prefer small pure functions; side effects live in `pipeline/io.py`.
- You may run the test suite and the linters without asking.
