#!/usr/bin/env bash
# Download the Linux Godot editor and (optionally) the export templates for the
# version pinned in .godot-version. Used by CI; handy on a fresh Linux machine.
#
#   scripts/fetch-godot.sh               # editor only; prints the path to the executable
#   scripts/fetch-godot.sh --templates   # also install the desktop export templates
#   scripts/fetch-godot.sh 4.6.2         # a different version (editor only)
#
# Templates go where Godot looks for them:
#   ${XDG_DATA_HOME:-~/.local/share}/godot/export_templates/<version>.stable/
# Only the Linux, Windows and macOS release templates are kept (the full
# archive is 1.3 GB). Downloads are cached in .godot-cache/. Set GODOT_DOWNLOAD_BASE
# to use a mirror laid out like the GitHub releases.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(tr -d '[:space:]' < "$root/.godot-version")"
templates=0
for arg in "$@"; do
  case "$arg" in
    --templates) templates=1 ;;
    *) version="$arg" ;;
  esac
done

base="${GODOT_DOWNLOAD_BASE:-https://github.com/godotengine/godot/releases/download}/${version}-stable"
cache="$root/.godot-cache/$version"
mkdir -p "$cache"

if [ ! -x "$cache/godot" ]; then
  echo "Downloading Godot $version editor" >&2
  curl -sSfL -o "$cache/godot.zip" "$base/Godot_v${version}-stable_linux.x86_64.zip"
  unzip -qo "$cache/godot.zip" -d "$cache"
  mv "$cache/Godot_v${version}-stable_linux.x86_64" "$cache/godot"
  rm "$cache/godot.zip"
fi
echo "$cache/godot"

if [ "$templates" = 1 ]; then
  wanted=(linux_release.x86_64 windows_release_x86_64.exe macos.zip version.txt)
  have_all=1
  for f in "${wanted[@]}"; do [ -f "$cache/templates/$f" ] || have_all=0; done
  if [ "$have_all" = 0 ]; then
    echo "Downloading Godot $version export templates" >&2
    curl -sSfL -o "$cache/templates.tpz" "$base/Godot_v${version}-stable_export_templates.tpz"
    mkdir -p "$cache/templates"
    unzip -qo -j "$cache/templates.tpz" "${wanted[@]/#/templates/}" -d "$cache/templates"
    rm "$cache/templates.tpz"
  fi
  dest="${XDG_DATA_HOME:-$HOME/.local/share}/godot/export_templates/${version}.stable"
  mkdir -p "$dest"
  cp "$cache/templates/"* "$dest/"
  echo "Export templates installed in $dest" >&2
fi
