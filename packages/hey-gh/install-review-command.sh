#!/bin/sh
# Install only on machines that use hey-gh. Preserve a prior eval wrapper.
set -eu
dir="$HOME/.local/bin"
dispatch="$dir/.hey-gh-dispatch"
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
command -v hey-gh >/dev/null
mkdir -p "$dispatch"
if [ ! -x "$dispatch/native" ]; then
  native=$(which -a gh | while IFS= read -r candidate; do
    case "$(file -Lb "$candidate")" in *Mach-O*|*ELF*) printf '%s\n' "$candidate"; break;; esac
  done)
  [ -n "$native" ] || { echo "Install native GitHub CLI first." >&2; exit 1; }
  if [ "$native" = "$dir/gh" ]; then
    cp -pL "$native" "$dispatch/native"
  else
    ln -s "$native" "$dispatch/native"
  fi
  if [ -L "$dir/gh" ]; then
    ln -s "$(realpath "$dir/gh")" "$dispatch/previous"
  elif [ -f "$dir/gh" ]; then
    cp -p "$dir/gh" "$dispatch/previous"
  fi
fi
install -m 755 "$source_dir/gh" "$dispatch/gh.new"
mv -f "$dispatch/gh.new" "$dir/gh"
