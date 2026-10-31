#!/usr/bin/env python3
"""Forwarder: the model adapter lives in adapters/models/openai_compat.py (it is a product piece,
not a bench one). Kept at this path because the bench and the nightly workflow call it."""
import os
import runpy

runpy.run_path(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "adapters", "models", "openai_compat.py"),
    run_name="__main__",
)
