#!/usr/bin/env bash
# Install the xcode build installed on this machine (~/.local/lib/xcode/bin, written by
# `make install`) on another device, replacing each file atomically so running sessions
# keep their old executable. No copies of replaced binaries are kept.
#
# On the target it writes ~/.local/lib/xcode/{bin/codex,bin/codex-code-mode-host,
# bin/codex.xcode.json,bin/xcode -> codex,LICENSE,NOTICE} and links ~/.local/bin/xcode to
# ~/.local/lib/xcode/bin/codex. It never touches Harmony's agent link or another folder.
#
# Usage: scripts/install-xcode-remote.sh omega|delta
#   omega: ssh omega
#   delta: through Omega (Alpha has no host key for Delta), wao@192.168.100.97
set -euo pipefail

target=${1:?usage: install-xcode-remote.sh omega|delta}
src="$HOME/.local/lib/xcode/bin"
files=(codex codex-code-mode-host codex.xcode.json ../LICENSE ../NOTICE)
shas=()
for f in "${files[@]}"; do shas+=("$(sha256sum "$src/$f" | cut -d' ' -f1)"); done

# Runs on the target: verify the staged files, keep each file's mode, rename into place,
# then publish the `xcode` commands as links.
read -r -d '' finish <<'EOF' || true
set -eu
d="$HOME/.local/lib/xcode/bin"; cd "$d"
i=0
for f in codex codex-code-mode-host codex.xcode.json ../LICENSE ../NOTICE; do
  i=$((i + 1)); want=$(eval echo "\${$i}")
  [ "$(sha256sum "$f.xcode-new" | cut -d' ' -f1)" = "$want" ] || { echo "checksum mismatch: $f" >&2; exit 1; }
  if [ -e "$f" ]; then chmod --reference="$f" "$f.xcode-new"
  else case "$f" in codex|codex-code-mode-host) chmod 755 "$f.xcode-new" ;; *) chmod 644 "$f.xcode-new" ;; esac; fi
done
mv -f codex-code-mode-host.xcode-new codex-code-mode-host
mv -f codex.xcode-new codex
for f in codex.xcode.json ../LICENSE ../NOTICE; do mv -f "$f.xcode-new" "$f"; done
if [ ! -L xcode ]; then ln -sfn codex xcode.xcode-new; mv -fT xcode.xcode-new xcode; fi
mkdir -p "$HOME/.local/bin"
ln -sfn "$d/codex" "$HOME/.local/bin/xcode.xcode-new"; mv -fT "$HOME/.local/bin/xcode.xcode-new" "$HOME/.local/bin/xcode"
echo "$(hostname): $(sha256sum codex | cut -c1-16) $("$HOME/.local/bin/xcode" --version)"
EOF

stage='mkdir -p ~/.local/lib/xcode/bin'
case "$target" in
  omega)
    ssh -o BatchMode=yes omega "$stage"
    for f in "${files[@]}"; do scp -q "$src/$f" "omega:.local/lib/xcode/bin/$f.xcode-new"; done
    ssh -o BatchMode=yes omega bash -s "${shas[@]}" <<<"$finish"
    ;;
  delta)
    # Relay through Omega's installed copy, which must already match this machine's.
    remote=$(ssh -o BatchMode=yes omega "sha256sum ~/.local/lib/xcode/bin/codex | cut -d' ' -f1")
    [ "$remote" = "${shas[0]}" ] || { echo "install on omega first" >&2; exit 1; }
    ssh -o BatchMode=yes omega "ssh -o BatchMode=yes wao@192.168.100.97 '$stage' && cd ~/.local/lib/xcode/bin && for f in ${files[*]}; do scp -q -o BatchMode=yes \$f wao@192.168.100.97:.local/lib/xcode/bin/\$f.xcode-new; done"
    ssh -o BatchMode=yes omega "ssh -o BatchMode=yes wao@192.168.100.97 bash -s ${shas[*]}" <<<"$finish"
    ;;
  *) echo "unknown target: $target" >&2; exit 2 ;;
esac
