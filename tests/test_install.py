"""Installer regression tests; no root privileges or compiled core required."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.prefix = self.base / 'prefix with spaces'
        self.config = self.base / 'app config'
        self.binary = self.base / 'fixture-core'
        self.binary.write_text('#!/bin/sh\nprintf "installer fixture\\n"\n')
        self.binary.chmod(0o755)

    def install(self):
        return subprocess.run(
            ['bash', str(ROOT / 'scripts/install.sh'), '--binary', str(self.binary),
             '--prefix', str(self.prefix), '--config-dir', str(self.config)],
            capture_output=True, text=True, check=False,
        )

    def test_wrapper_replaces_symlink_without_changing_target(self):
        bindir = self.prefix / 'bin'
        bindir.mkdir(parents=True)
        target = self.base / 'unrelated.txt'
        target.write_text('preserve me\n')
        target.chmod(0o640)
        wrapper = bindir / 'dagger-setup'
        wrapper.symlink_to(target)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(target.read_text(), 'preserve me\n')
        self.assertEqual(target.stat().st_mode & 0o777, 0o640)
        self.assertFalse(wrapper.is_symlink())
        self.assertEqual(wrapper.stat().st_mode & 0o777, 0o755)
        self.assertEqual(list(bindir.glob('.dagger-setup.*')), [])

    def test_paths_with_spaces_and_existing_config_survive_reinstall(self):
        self.config.mkdir()
        config = self.config / 'server.json'
        config.write_text('{"fixture": true}\n')
        for _ in range(2):
            result = self.install()
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(config.read_text(), '{"fixture": true}\n')
        setup = self.prefix / 'share/dagger-rs/setup.sh'
        setup.write_text('printf "%s\\n%s\\n" "$DAGGER_BIN" "$DAGGER_CONFIG_DIR"\n')
        result = subprocess.run([str(self.prefix / 'bin/dagger-setup')],
                                capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout.splitlines(),
                         [str(self.prefix / 'bin/dagger-rs'), str(self.config)])

    def test_directory_at_wrapper_path_is_rejected_without_nesting(self):
        wrapper = self.prefix / 'bin/dagger-setup'
        wrapper.mkdir(parents=True)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(wrapper.iterdir()), [])
        self.assertEqual(list(wrapper.parent.glob('.dagger-setup.*')), [])


if __name__ == '__main__':
    unittest.main()
