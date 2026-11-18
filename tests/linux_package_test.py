"""Exercise the actual Linux download after relocation, including a custom cell."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
NAME = 'xcode-linux-x86_64'


class LinuxPackageTest(unittest.TestCase):
    def test_relocated_download_runs_cli_and_native_custom_cells(self):
        parent = ROOT / 'build/linux-package-tests'
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=parent, prefix='relocated with spaces-') as temporary:
            directory = Path(temporary)
            with tarfile.open(ROOT / 'build' / (NAME + '.tar.gz')) as archive:
                archive.extractall(directory, filter='data')
            package = directory / NAME
            alias = directory / 'xcode'
            alias.symlink_to(package / 'xcode')
            env = {key: value for key, value in os.environ.items() if key not in
                   ['DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'DBUS_SESSION_BUS_ADDRESS',
                    'XCODE_ZIRAN_TOOLCHAIN', 'XCODE_TEST_CLI', 'XCODE_TEST_HOST']}
            metadata = subprocess.check_output([str(alias), '--xcode-runtime-info'], env=env, text=True)
            self.assertEqual(json.loads(metadata)['product'], 'xcode')
            env.update(XCODE_TEST_CLI=str(alias),
                       XCODE_TEST_HOST=str(package / 'libexec/codex-code-mode-host'),
                       XCODE_ZIRAN_TOOLCHAIN=str(package / 'toolchain'))
            for name in ['native_cli_test.py', 'native_host_test.py']:
                subprocess.run([sys.executable, str(ROOT / 'tests' / name)],
                               cwd=directory, env=env, check=True)


if __name__ == '__main__':
    unittest.main()
