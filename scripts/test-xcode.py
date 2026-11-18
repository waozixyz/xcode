#!/usr/bin/env python3
"""Run the complete local suite and certify only the binaries it tested."""
import argparse
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
RECEIPT = ROOT / 'backup/xcode-validation.json'
PROFILE = os.environ.get('XCODE_CARGO_PROFILE', 'local')
installer = runpy.run_path(str(ROOT / 'scripts/install-xcode.py'))
digest = installer['digest']
# Native tests built by `make native-test-build`, by suite name.
NATIVE_TESTS = {'abi': 'native-abi-test', 'allocator': 'native-allocator-test', 'idle': 'native-idle-test'}
# Every Rust library suite in the local suite, with the source it is built from.
RUST_SUITES = {'codex-api': 'codex-rs/codex-api',
               'codex-code-mode-runtime': 'codex-rs/code-mode-runtime',
               'codex-code-mode-protocol': 'codex-rs/code-mode-protocol',
               'codex-mermaid': 'codex-rs/mermaid',
               'codex-utils-fuzzy-match': 'codex-rs/utils/fuzzy-match'}


def test_source_hashes():
    files = [ROOT / name for name in ['codex-rs/Cargo.toml', 'codex-rs/Cargo.lock',
                                     'tests/native_abi.rs', 'tests/native_allocator.rs',
                                     'tests/native_idle.rs']]
    for directory in ['native', *RUST_SUITES.values()]:
        files.extend(p for p in (ROOT / directory).rglob('*') if p.suffix in ('.rs', '.zi', '.toml') and p.is_file())
    return {p.relative_to(ROOT).as_posix(): digest(p) for p in sorted(files)}


def prebuilt_tests():
    path = ROOT / 'build/native-dispatch/tests.json'
    value = json.loads(path.read_text())
    if value.get('schema') != 1 or value.get('sources') != test_source_hashes():
        raise RuntimeError('Prebuilt local tests do not match the current source')
    expected = {*NATIVE_TESTS, *RUST_SUITES}
    rows = value.get('tests', [])
    if len(rows) != len(expected) or {row['suite'] for row in rows} != expected:
        raise RuntimeError('Prebuilt manifest must contain every required local-test suite')
    for row in rows:
        relative = Path(row['path'])
        allowed = ('build/native-dispatch' if row['suite'] in NATIVE_TESTS
                   else 'codex-rs/target/' + PROFILE + '/deps')
        if relative.is_absolute() or '..' in relative.parts or relative.parent != Path(allowed):
            raise RuntimeError('Unsafe prebuilt local-test path')
        if row['suite'] in NATIVE_TESTS:
            if relative.name != NATIVE_TESTS[row['suite']]:
                raise RuntimeError('Wrong native local-test executable')
        elif not relative.name.startswith(row['suite'].replace('-', '_') + '-'):
            raise RuntimeError('Wrong Rust local-test executable')
        binary = ROOT / relative
        if binary.is_symlink() or not binary.is_file() or digest(binary) != row['sha256']:
            raise RuntimeError('Prebuilt local-test executable changed')
    return value


def fingerprints():
    return installer['packaged_digests'](ROOT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cargo', required=True)
    parser.add_argument('--ui-features', default='')
    parser.add_argument('--existing-binaries', action='store_true',
                        help='Test the existing packaged executables without rebuilding products; all local test suites still run')
    parser.add_argument('--prebuilt-tests', action='store_true',
                        help='Run every previously built local-test executable outside the build namespace; verify source and executable hashes')
    args = parser.parse_args()
    subprocess.run(['git', 'check-ignore', '-q', str(RECEIPT)], cwd=ROOT, check=True)
    RECEIPT.parent.mkdir(exist_ok=True, mode=0o700)
    # A failed rerun must never leave an earlier success available to install.
    RECEIPT.unlink(missing_ok=True)
    env = {key: value for key, value in os.environ.items() if key not in
           ['DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'DBUS_SESSION_BUS_ADDRESS',
            'XCODE_TEST_HOST', 'XCODE_TEST_CLI']}
    env['CARGO'] = args.cargo
    manifest = prebuilt_tests() if args.prebuilt_tests else None
    if args.prebuilt_tests and not args.existing_binaries:
        raise ValueError('--prebuilt-tests requires --existing-binaries')
    if manifest is None:
        # The native cell test runs the packaged host, so build it first.
        targets = ['native-test'] if args.existing_binaries else ['all', 'native-test']
        subprocess.run(['make', *targets, 'UI_FEATURES=' + args.ui_features],
                       cwd=ROOT, env=env, check=True)
    tested = fingerprints()
    runtime = ROOT / 'build/native-runtime'
    runtime.mkdir(exist_ok=True, mode=0o700)
    runtime.chmod(0o700)
    env['XCODE_RUNTIME_DIR'] = str(runtime)
    commands = ([
        [str(ROOT / row['path'])] for row in manifest['tests']
    ] + [[sys.executable, 'tests/native_cell_test.py']] if manifest is not None else [
        [args.cargo, 'test', '--manifest-path', 'codex-rs/Cargo.toml', '--profile', PROFILE,
         '-p', package, '--lib']
        for package in RUST_SUITES
    ])
    commands += [[sys.executable, 'tests/' + name]
                 for name in ['validation_test.py', 'native_host_test.py', 'native_cli_test.py', 'install_xcode_test.py']]
    for command in commands:
        subprocess.run(command, cwd=ROOT, env=env, check=True)
    if tested != fingerprints():
        raise RuntimeError('Packaged binaries changed during local tests; rerun make test')
    if manifest is not None and manifest != prebuilt_tests():
        raise RuntimeError('Prebuilt local tests changed during execution')
    receipt = dict(local_tests_passed=True, tested_at=int(time.time()), **tested)
    descriptor, temporary = tempfile.mkstemp(dir=RECEIPT.parent, prefix='.xcode-validation-')
    try:
        with os.fdopen(descriptor, 'w') as stream:
            json.dump(receipt, stream, indent=2)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, RECEIPT)
    finally:
        Path(temporary).unlink(missing_ok=True)
    print('Local suite passed; installation receipt saved for the tested binaries.', flush=True)


if __name__ == '__main__':
    main()
