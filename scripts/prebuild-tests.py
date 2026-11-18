#!/usr/bin/env python3
"""Build local-test executables for a separate host execution phase.

Cell tests must run outside the build namespace so their production sandboxes
can create their own namespaces. No tests are skipped by the host phase.
"""
import argparse
import json
from pathlib import Path
import runpy
import subprocess

ROOT = Path(__file__).resolve().parents[1]
validation = runpy.run_path(str(ROOT / 'scripts/test-xcode.py'))
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cargo', required=True)
args = parser.parse_args()
path = ROOT / 'build/native-dispatch/tests.json'
path.unlink(missing_ok=True)
before = validation['test_source_hashes']()
packages = [argument for package in validation['RUST_SUITES'] for argument in ('-p', package)]
result = subprocess.run([args.cargo, 'test', '--manifest-path', 'codex-rs/Cargo.toml',
                         '--profile', validation['PROFILE'], '--offline', '--no-run', '--lib', '--message-format=json',
                         *packages],
                        cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True)
tests = {'abi': 'build/native-dispatch/native-abi-test',
         'allocator': 'build/native-dispatch/native-allocator-test',
         'idle': 'build/native-dispatch/native-idle-test'}
names = {package.replace('-', '_'): package for package in validation['RUST_SUITES']}
for line in result.stdout.splitlines():
    item = json.loads(line)
    if item.get('reason') != 'compiler-artifact' or not item.get('executable'):
        continue
    suite = names.get(item['target']['name'])
    if suite and item['profile']['test'] and 'lib' in item['target']['kind']:
        tests[suite] = Path(item['executable']).resolve().relative_to(ROOT).as_posix()
if set(tests) != {'abi', 'allocator', 'idle', *names.values()}:
    raise RuntimeError('Cargo did not report every required local-test executable')
if before != validation['test_source_hashes']():
    raise RuntimeError('Source changed while compiling local tests')
value = {'schema': 1, 'sources': before, 'tests': [
    {'suite': suite, 'path': name, 'sha256': validation['digest'](ROOT / name)}
    for suite, name in sorted(tests.items())]}
path.write_text(json.dumps(value, indent=2) + '\n')
print('Prebuilt every native local-test suite; host execution is still required.')
