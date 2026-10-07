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

    def test_classic_menu_installs_server_with_numeric_transport(self):
        import json
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        keys = self.config / 'server-keys'
        keys.mkdir()
        (keys / 'private.key').write_text('fixture')
        (keys / 'public.key').write_text('b' * 64)
        inputs = '1\n' + 'a' * 64 + '\n1\n' + '\n' * 5 + 'no\n\n0\n'
        result = subprocess.run([str(self.prefix / 'bin/dagger-setup-classic')],
                                input=inputs, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        config = json.loads((self.config / 'server.json').read_text())
        self.assertEqual(config['mode'], 'server')
        self.assertEqual(config['listeners'][0]['transport'], 'tcp')
        self.assertEqual(config['listeners'][0]['maps'][0]['target'], '127.0.0.1:8080')
        self.assertNotIn('Role (server/client)', result.stdout)

    def test_classic_menu_can_open_previous_menu(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run([str(self.prefix / 'bin/dagger-setup-classic')],
                                input='12\n0\n\n0\n', capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Dagger Rust — local Linux setup', result.stdout)
        self.assertTrue((self.prefix / 'bin/dagger-setup').exists())




class ReleaseInstallerTests(unittest.TestCase):
    def test_release_checksum_controls_execution(self):
        import hashlib
        import json
        import os

        for valid in (True, False):
            with self.subTest(valid_checksum=valid), tempfile.TemporaryDirectory() as td:
                base = Path(td)
                shim = base / 'shim'
                shim.mkdir()
                fixture = base / 'fixture'
                fixture.mkdir()
                marker = base / 'executed'
                payload = b'#!/usr/bin/env bash\nprintf installed > "$INSTALL_TEST_MARKER"\n'
                name = 'dagger-rs-linux-x86_64.run'
                (fixture / name).write_bytes(payload)
                digest = hashlib.sha256(payload if valid else b'wrong payload').hexdigest()
                (fixture / (name + '.sha256')).write_text(digest + '  ' + name + '\n')
                (fixture / 'release.json').write_text(json.dumps({
                    'tag_name': 'v0.2.1-v2grop.2',
                    'assets': [{'name': name}, {'name': name + '.sha256'}],
                }))
                (shim / 'curl').write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
url = next(a for a in args if a.startswith('https://'))
name = 'release.json' if url.endswith('/releases/latest') else url.rsplit('/', 1)[-1]
shutil.copyfile(pathlib.Path(os.environ['INSTALL_TEST_FIXTURE']) / name,
                args[args.index('-o') + 1])
''')
                (shim / 'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
                (shim / 'sudo').write_text('#!/bin/sh\nexec "$@"\n')
                for path in shim.iterdir():
                    path.chmod(0o755)
                env = dict(os.environ, PATH=str(shim) + os.pathsep + os.environ['PATH'],
                           INSTALL_TEST_FIXTURE=str(fixture), INSTALL_TEST_MARKER=str(marker),
                           TMPDIR=str(base))
                result = subprocess.run(['bash', str(ROOT / 'scripts/install-release.sh')],
                                        env=env, capture_output=True, text=True)
                if valid:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(marker.read_text(), 'installed')
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(marker.exists())


if __name__ == '__main__':
    unittest.main()
