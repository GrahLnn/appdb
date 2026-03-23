#!/usr/bin/env sh
set -eu

# Windows note: workers/orchestrators should read this file as setup intent and
# execute the equivalent steps with Windows-native commands instead of launching
# the .sh file directly.
# Mission setup remains dependency-only; runtime tests use the repo's embedded
# SurrealDB fixtures and do not require extra services.
cargo fetch
