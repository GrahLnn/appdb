#!/usr/bin/env bash
set -e
# Windows sessions must read this file as documentation only.
# Idempotent worker setup actions:
# 1. Run `cargo fetch --locked` from the repo root to ensure dependencies are present.
# 2. Inspect `git status --short` before editing because this repo may start with intentional dirty files.
# 3. Use workspace-local cargo commands from `.factory/services.yaml` for validation.
