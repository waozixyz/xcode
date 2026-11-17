#!/usr/bin/env bash
# Install the xcode build installed on this machine (~/.local/lib/xcoder/bin, written by
# `make install`) on another device, replacing each file atomically so running sessions
# keep their old executable. No copies of replaced binaries are kept.
#
# Usage: scripts/install-xcoder-remote.sh omega|delta
#   omega: ssh omega
#   delta: through Omega (Alpha has no host key for Delta), wao@192.168.100.97
set -euo pipefail

target=${1:?usage: install-xcoder-remote.sh omega|delta}
src="$HOME/.local/lib/xcoder/bin"
files=(codex codex-code-mode-host codex.xcoder.json)
shas=()
for f in "${files[@]}"; do shas+=("$(sha256sum "$src/$f" | cut -d' ' -f1)"); done

# Runs on the target: verify the staged files, keep each file's mode, rename into place,
# and make `xcode` a link to `codex` when it is still a separate copy.
read -r -d '' finish <<'EOF' || true
set -eu
d="$HOME/.local/lib/xcoder/bin"; cd "$d"
i=0
for f in codex codex-code-mode-host codex.xcoder.json; do
  i=$((i + 1)); want=$(eval echo "\${$i}")
  [ "$(sha256sum "$f.xcoder-new" | cut -d' ' -f1)" = "$want" ] || { echo "checksum mismatch: $f" >&2; exit 1; }
  [ -e "$f" ] && chmod --reference="$f" "$f.xcoder-new"
done
mv -f codex-code-mode-host.xcoder-new codex-code-mode-host
mv -f codex.xcoder-new codex
mv -f codex.xcoder.json.xcoder-new codex.xcoder.json
if [ -f xcode ] && [ ! -L xcode ]; then ln -s codex xcode.xcoder-new; mv -fT xcode.xcoder-new xcode; fi
echo "$(hostname): $(sha256sum codex | cut -c1-16) $("$d/xcode" --version)"
EOF

case "$target" in
  omega)
    for f in "${files[@]}"; do scp -q "$src/$f" "omega:.local/lib/xcoder/bin/$f.xcoder-new"; done
    ssh -o BatchMode=yes omega bash -s "${shas[@]}" <<<"$finish"
    ;;
  delta)
    # Relay through Omega's installed copy, which must already match this machine's.
    remote=$(ssh -o BatchMode=yes omega "sha256sum ~/.local/lib/xcoder/bin/codex | cut -d' ' -f1")
    [ "$remote" = "${shas[0]}" ] || { echo "install on omega first" >&2; exit 1; }
    ssh -o BatchMode=yes omega "cd ~/.local/lib/xcoder/bin && for f in ${files[*]}; do scp -q -o BatchMode=yes \$f wao@192.168.100.97:.local/lib/xcoder/bin/\$f.xcoder-new; done"
    ssh -o BatchMode=yes omega "ssh -o BatchMode=yes wao@192.168.100.97 bash -s ${shas[*]}" <<<"$finish"
    ;;
  *) echo "unknown target: $target" >&2; exit 2 ;;
esac
