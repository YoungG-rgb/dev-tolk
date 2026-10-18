#!/usr/bin/env bash
# Собирает tolk-engine (release) и кладёт его как сайдкар приложения Tauri:
# app/src-tauri/binaries/tolk-engine-<target triple>[.exe] (TD-04, MVP-013).
# Запускать перед `cargo build/clippy` приложения и перед `tauri build`: tauri-build проверяет наличие файла.
set -euo pipefail
cd "$(dirname "$0")/.."
triple="$(rustc -vV | sed -n 's/^host: //p')"
ext=""
case "$triple" in *windows*) ext=".exe" ;; esac
cargo build --release -p tolk-engine
mkdir -p app/src-tauri/binaries
cp "target/release/tolk-engine${ext}" "app/src-tauri/binaries/tolk-engine-${triple}${ext}"
echo "сайдкар: app/src-tauri/binaries/tolk-engine-${triple}${ext}"

# Модель определения речи (Silero VAD, ~1 МБ) кладётся в ресурсы приложения: устанавливается из .app без сети.
cargo run --release -q -p tolk-core --example prepare_vad -- app/src-tauri/resources
