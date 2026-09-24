#!/usr/bin/env bash
# Build the release library and export the packaged app with Godot.
#
#   scripts/package.sh                      # this OS only
#   scripts/package.sh linux windows macos  # several (needs each OS's library)
#   LIB_DIR=dist scripts/package.sh all     # use prebuilt libraries (CI)
#
# Output: build/<platform>/ (the exported app) and build/OpenTerrainStudio-<version>-<platform>.zip
#
# Environment:
#   GODOT    Godot executable (default: `godot`). Must be exactly the version in
#            .godot-version, with matching export templates installed
#            (scripts/fetch-godot.sh downloads both).
#   LIB_DIR  Folder with prebuilt release libraries (libterrain_godot.so,
#            terrain_godot.dll, libterrain_godot.dylib). If unset, the library
#            for this OS is built with `cargo build --release`.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

GODOT="${GODOT:-godot}"
godot_version="$(tr -d '[:space:]' < .godot-version)"
app_version="$(sed -n 's/^config\/version="\(.*\)"/\1/p' app/project.godot)"

die() { echo "package.sh: $*" >&2; exit 1; }

case "$(uname -s)" in
  Linux*) host=linux ;;
  Darwin*) host=macos ;;
  MINGW* | MSYS* | CYGWIN*) host=windows ;;
  *) die "unsupported OS $(uname -s)" ;;
esac

platforms=("$@")
[ ${#platforms[@]} -eq 0 ] && platforms=("$host")
[ "${platforms[0]}" = all ] && platforms=(linux windows macos)

# platform -> export preset name, library file, zip suffix
preset() { case "$1" in linux) echo Linux ;; windows) echo Windows ;; macos) echo macOS ;; *) die "unknown platform $1" ;; esac; }
libfile() { case "$1" in linux) echo libterrain_godot.so ;; windows) echo terrain_godot.dll ;; macos) echo libterrain_godot.dylib ;; esac; }
suffix() { case "$1" in linux) echo linux-x86_64 ;; windows) echo windows-x86_64 ;; macos) echo macos-universal ;; esac; }

# ---- Godot version check -----------------------------------------------------
command -v "$GODOT" > /dev/null || die "Godot not found; set GODOT=/path/to/godot (version $godot_version)"
have="$("$GODOT" --headless --version 2>/dev/null | tail -n 1)"
case "$have" in
  "$godot_version".stable*) ;;
  *) die "Godot $godot_version is required (export templates must match exactly), found '$have'" ;;
esac

# ---- Release libraries into app/bin ------------------------------------------
mkdir -p app/bin
for p in "${platforms[@]}"; do
  lib="$(libfile "$p")"
  if [ -n "${LIB_DIR:-}" ]; then
    [ -f "$LIB_DIR/$lib" ] || die "$LIB_DIR/$lib missing (needed for $p)"
    [ "$LIB_DIR/$lib" -ef "app/bin/$lib" ] || cp "$LIB_DIR/$lib" app/bin/
  elif [ "$p" = "$host" ]; then
    if [ "$p" = macos ]; then
      # Universal library: both architectures, merged with lipo.
      rustup target add aarch64-apple-darwin x86_64-apple-darwin > /dev/null
      cargo build --release -p terrain-godot --target aarch64-apple-darwin
      cargo build --release -p terrain-godot --target x86_64-apple-darwin
      lipo -create -output "app/bin/$lib" \
        "target/aarch64-apple-darwin/release/$lib" "target/x86_64-apple-darwin/release/$lib"
    else
      cargo build --release -p terrain-godot
      cp "target/release/$lib" app/bin/
    fi
  else
    die "can't build the $p library on $host; build it there and pass LIB_DIR"
  fi
done

# ---- Export ------------------------------------------------------------------
# Import first: on a fresh checkout Godot (4.6, 4.7) segfaults on exit after the
# first import. The import itself completes, so ignore the exit status here and
# export from the already-imported project.
if [ ! -d app/.godot/imported ]; then
  "$GODOT" --headless --path app --import > /dev/null 2>&1 || true
fi

mkdir -p build
for p in "${platforms[@]}"; do
  name="OpenTerrainStudio-$app_version-$(suffix "$p")"
  out="build/$p"
  rm -rf "$out" && mkdir -p "$out"
  case "$p" in
    linux) target="$out/OpenTerrainStudio.x86_64" ;;
    windows) target="$out/OpenTerrainStudio.exe" ;;
    macos) target="$out/OpenTerrainStudio.zip" ;;
  esac
  echo "==> Exporting $(preset "$p") to $target"
  # Without a debug build in target/debug the editor can't load the extension
  # and logs "GDExtension dynamic library not found" and script parse errors
  # while exporting. They're harmless: exporting doesn't run the extension.
  "$GODOT" --headless --path app --export-release "$(preset "$p")" "$root/$target"
  [ -f "$target" ] || die "export for $p produced no $target"

  zip="build/$name.zip"
  rm -f "$zip"
  if [ "$p" = macos ]; then
    # Godot already zips the .app bundle; add the licences next to it.
    mv "$target" "$zip"
    zip -qj "$zip" LICENSE-MIT LICENSE-APACHE
  else
    [ -f "$out/$(libfile "$p")" ] || die "the exported $p app is missing $(libfile "$p")"
    cp LICENSE-MIT LICENSE-APACHE "$out/"
    (cd build && rm -rf "$name" && cp -r "$p" "$name" && zip -qr "$name.zip" "$name" && rm -rf "$name")
  fi
  echo "==> $zip"
done
