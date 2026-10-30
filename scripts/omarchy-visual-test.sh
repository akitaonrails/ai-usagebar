#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$repo_root/target/omarchy-visual-scenarios"}
shell_root=${OMARCHY_SHELL_DIR:-"${XDG_DATA_HOME:-$HOME/.local/share}/omarchy/shell"}

if ! command -v quickshell >/dev/null 2>&1; then
  echo "quickshell is required for Omarchy visual scenarios" >&2
  exit 1
fi
for module in Commons Ui; do
  if [[ ! -d "$shell_root/$module" ]]; then
    echo "Omarchy shell module not found: $shell_root/$module" >&2
    echo "Set OMARCHY_SHELL_DIR to the shell source/install directory." >&2
    exit 1
  fi
done

mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/ai-usagebar-visual.XXXXXX")
cleanup() {
  [[ -n ${scratch:-} && -d $scratch ]] && rm -rf -- "$scratch"
}
trap cleanup EXIT
capture_dir="$scratch/output"
mkdir -- "$capture_dir"

ln -s -- "$shell_root/Commons" "$scratch/Commons"
ln -s -- "$shell_root/Ui" "$scratch/Ui"
ln -s -- "$repo_root/omarchy" "$scratch/Usage"
ln -s -- "$repo_root/omarchy/visual-tests/shell.qml" "$scratch/shell.qml"

AI_USAGEBAR_VISUAL_OUTPUT_DIR="$capture_dir" \
QT_QPA_PLATFORM=offscreen \
timeout 15 quickshell -p "$scratch" --no-color \
  --log-rules 'qt.qml.connections=false'

# The whole panel, from a synthetic report. qs.Ui's KeyboardPanel is a
# layer-shell window, so this config gets a Ui module whose KeyboardPanel is
# the offscreen stand-in. `ai-usagebar-<weekly>` are links to the stub; a bare
# `ai-usagebar` on PATH fails, so the real binary and its credentials are never
# reached. The brand marks are a shader effect, which needs an RHI backend:
# OpenGL by default, or AI_USAGEBAR_VISUAL_BACKEND=software without a GPU
# (the logos are then blank).
panel_root="$scratch/panel"
mkdir -- "$panel_root" "$panel_root/Ui" "$panel_root/bin"
for file in "$shell_root"/Ui/*; do
  [[ $(basename -- "$file") == KeyboardPanel.qml ]] && continue
  ln -s -- "$file" "$panel_root/Ui/"
done
ln -s -- "$repo_root/omarchy/visual-tests/KeyboardPanel.qml" "$panel_root/Ui/KeyboardPanel.qml"
ln -s -- "$shell_root/Commons" "$panel_root/Commons"
ln -s -- "$repo_root/omarchy" "$panel_root/Usage"
ln -s -- "$repo_root/omarchy/visual-tests/panel.qml" "$panel_root/shell.qml"
for weekly in 69 94 100; do
  ln -s -- "$repo_root/omarchy/visual-tests/fake-ai-usagebar" "$panel_root/bin/ai-usagebar-$weekly"
done
printf '#!/bin/sh\necho "visual scenarios never run the real ai-usagebar" >&2\nexit 127\n' \
  > "$panel_root/bin/ai-usagebar"
chmod +x -- "$panel_root/bin/ai-usagebar"

backend=${AI_USAGEBAR_VISUAL_BACKEND:-opengl}
if [[ $backend == software ]]; then
  render_env=(QT_QUICK_BACKEND=software)
else
  render_env=(QT_QUICK_BACKEND=rhi "QSG_RHI_BACKEND=$backend")
fi
env "${render_env[@]}" \
PATH="$panel_root/bin:$PATH" \
AI_USAGEBAR_VISUAL_OUTPUT_DIR="$capture_dir" \
AI_USAGEBAR_VISUAL_BIN_DIR="$panel_root/bin" \
QT_QPA_PLATFORM=offscreen \
timeout 60 quickshell -p "$panel_root" --no-color \
  --log-rules 'qt.qml.connections=false'

for image in settings-display-dark.png settings-credentials-light.png \
  notices-dark.png notices-light-pt-BR.png settings-priorities-dark.png \
  priorities-dark.png priorities-light-pt-BR.png \
  panel-dark.png panel-names-dark.png panel-light-pt-BR.png \
  panel-near-limit-dark.png panel-exhausted-dark.png \
  panel-exhausted-colors-dark.png panel-balance-dark.png \
  panel-priorities-dark.png; do
  if [[ ! -s "$capture_dir/$image" ]]; then
    echo "Visual scenario did not produce $image" >&2
    exit 1
  fi
  install -m 0644 -- "$capture_dir/$image" "$output_dir/$image"
done

echo "Omarchy visual scenarios: $output_dir"
