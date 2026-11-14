#!/usr/bin/env python3
"""Archive the locally tested Linux executables with a relocatable Ziran toolchain."""
import argparse
from pathlib import Path
import runpy
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
NAME = 'xcoder-linux-x86_64'
installer = runpy.run_path(str(ROOT / 'scripts/install-xcoder.py'))
digest = installer['digest']


def package(toolchain):
    tested = installer['tested_digests']()
    toolchain = toolchain.resolve(strict=True)
    for name in ['build/bin/ziran', 'build/bin/zi2c', 'std', 'include', 'LICENSE']:
        if not (toolchain / name).exists():
            raise ValueError('Incomplete Ziran toolchain: ' + name)
    output = ROOT / 'build'
    with tempfile.TemporaryDirectory(dir=output, prefix='.linux-package-') as temporary:
        stage = Path(temporary) / NAME
        (stage / 'libexec').mkdir(parents=True)
        for key, name in installer['PACKAGED'].items():
            copied = stage / 'libexec' / name
            shutil.copy2(ROOT / 'build/bin' / name, copied)
            if digest(copied) != tested[key]:
                raise ValueError('Tested executable changed while packaging: ' + name)
        for name in ['LICENSE', 'NOTICE']:
            shutil.copy2(ROOT / name, stage / name)
        shutil.copy2(ROOT / 'docs/linux-builds.md', stage / 'README.md')
        for name in ['build/bin', 'std', 'include']:
            shutil.copytree(toolchain / name, stage / 'toolchain' / name)
        shutil.copy2(toolchain / 'LICENSE', stage / 'toolchain/LICENSE')
        launcher = stage / 'xcoder'
        launcher.write_text('''#!/bin/sh
set -eu
xcoder_root=$(CDPATH= cd -- "$(dirname -- "$(readlink -f -- "$0")")" && pwd)
export XCODER_ZIRAN_TOOLCHAIN="${XCODER_ZIRAN_TOOLCHAIN:-"$xcoder_root/toolchain"}"
exec "$xcoder_root/libexec/xcoder" "$@"
''')
        launcher.chmod(0o755)
        archive = output / (NAME + '.tar.gz')
        staged_archive = Path(temporary) / archive.name
        with tarfile.open(staged_archive, 'w:gz') as bundle:
            bundle.add(stage, arcname=NAME)
        staged_archive.replace(archive)
    checksum = archive.with_name(archive.name + '.sha256')
    checksum.write_text(digest(archive) + '  ' + archive.name + '\n')
    print(archive)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--toolchain', type=Path)
    args = parser.parse_args()
    toolchain = args.toolchain or Path(subprocess.check_output(
        ['ziran', 'pkg', 'path', 'ziran'], cwd=ROOT, text=True).strip())
    package(toolchain)
