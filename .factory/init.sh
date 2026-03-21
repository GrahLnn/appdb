#!/usr/bin/env sh
set -eu

# Windows note: workers/orchestrators should read this file as setup intent and
# execute the equivalent steps with Windows-native commands instead of launching
# the .sh file directly.
cargo fetch
