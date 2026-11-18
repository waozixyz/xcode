"""Exercise atomic package cutover with a real, private running executable."""
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
installer = runpy.run_path(str(ROOT / 'scripts/install-xcode.py'))
publish = installer['publish_package_entry']


class PackageCutoverTest(unittest.TestCase):
    def setUp(self):
        parent = ROOT / 'build/install-tests'
        parent.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()

    def test_running_executable_keeps_its_inode_and_can_still_be_executed(self):
        entry = self.root / 'codex'
        compatibility = self.root / 'codex-upstream'
        native = self.root / 'xcode'
        # A plain shell, not coreutils: multi-call coreutils builds (uutils)
        # refuse to run under another file name.
        shell = os.path.realpath('/bin/sh')
        shutil.copy2(shell, entry)
        shutil.copy2(shell, native)
        inode = entry.stat().st_ino
        # The shell stays the running process while its child sleeps.
        process = subprocess.Popen([str(entry), '-c', 'sleep 60; :'])
        try:
            publish(entry, native, compatibility)
            self.assertEqual(compatibility.stat().st_ino, inode)
            self.assertEqual(Path(os.readlink(f'/proc/{process.pid}/exe')), compatibility)
            self.assertIsNone(process.poll())
            subprocess.run([str(entry), '-c', ':'], check=True, timeout=5)
            subprocess.run([f'/proc/{process.pid}/exe', '-c', ':'], check=True, timeout=5)
        finally:
            process.terminate()
            process.wait(timeout=5)

    def test_existing_compatibility_entry_is_preserved(self):
        entry = self.root / 'codex'
        compatibility = self.root / 'codex-upstream'
        native = self.root / 'xcode'
        entry.write_text('original')
        compatibility.write_text('preserve')
        native.write_text('native')
        with self.assertRaises(FileExistsError):
            publish(entry, native, compatibility)
        self.assertEqual(entry.read_text(), 'original')
        self.assertEqual(compatibility.read_text(), 'preserve')

    def test_installs_xcode_without_creating_a_public_codex_alias(self):
        source = self.root / 'source'
        (source / 'build/bin').mkdir(parents=True)
        metadata = dict(product='xcode', tool_language='ziran', pipeline='zi2c',
                        javascript_tool_runtime=False)
        binary = source / 'build/bin/xcode'
        binary.write_text('#!/usr/bin/env python3\nprint(' + repr(json.dumps(metadata)) + ')\n')
        binary.chmod(0o755)
        host = source / 'build/bin/codex-code-mode-host'
        shutil.copy2('/usr/bin/true', host)
        for name in ['LICENSE', 'NOTICE']:
            (source / name).write_text('local installation fixture\n')
        (source / 'backup').mkdir()
        receipt = dict(local_tests_passed=True, binary_sha256=installer['digest'](binary),
                       host_sha256=installer['digest'](host))
        (source / 'backup/xcode-validation.json').write_text(json.dumps(receipt))
        install = installer['install']
        for managed in [False, True]:
            with self.subTest(managed=managed):
                prefix = self.root / ('managed' if managed else 'standalone')
                managed_command = prefix / 'lib/harmony/agent-bin/codex'
                if managed:
                    managed_command.parent.mkdir(parents=True)
                    managed_command.symlink_to('/usr/bin/true')
                with patch.dict(install.__globals__, ROOT=source):
                    install(prefix)
                self.assertTrue((prefix / 'bin/xcode').is_symlink())
                self.assertFalse((prefix / 'bin/codex').exists())
                folder = prefix / 'lib/xcode/bin'
                self.assertEqual(os.readlink(folder / 'xcode'), 'codex')
                self.assertEqual(json.loads((folder / 'codex.xcode.json').read_text())['product'], 'xcode')
                self.assertFalse((prefix / 'lib/xcoder').exists())
                self.assertFalse((prefix / 'bin/xcoder').exists())
                if managed:
                    self.assertEqual(managed_command.resolve(), (prefix / 'bin/xcode').resolve())


if __name__ == '__main__':
    unittest.main()
