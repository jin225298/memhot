"""Run as root on Linux to exercise actual ownership and manifest fences."""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from release_bundle import bundle_check, sha, SDK_PINS, npm_environment, run_unit


@unittest.skipUnless(os.geteuid() == 0, 'release ownership checks require root')
class ReleaseManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='memhot-release-test-')
        self.root = Path(self.temp.name)
        self.agent = self.root / 'agent'
        for name in ('src', 'test'):
            (self.agent / name).mkdir(parents=True)
        (self.agent / 'package.json').write_text(json.dumps({'dependencies': SDK_PINS}))
        self.lock = {'packages': {f'node_modules/{name}': {
            'version': version, 'resolved': f'https://registry.npmjs.org/{name}/-/package-{version}.tgz',
            'integrity': 'sha512-fixture'} for name, version in SDK_PINS.items()}}
        (self.agent / 'package-lock.json').write_text(json.dumps(self.lock))
        (self.agent / 'src/main.mjs').write_text('export const fixture = true;\n')
        (self.agent / 'test/fixture.test.mjs').write_text('export {};\n')
        self.refresh()

    def tearDown(self):
        self.temp.cleanup()

    def refresh(self):
        files = {path.relative_to(self.root).as_posix(): sha(path)
                 for path in self.agent.rglob('*') if path.is_file()}
        self.manifest = self.root / 'release-manifest.json'
        self.manifest.write_text(json.dumps({'version': '0.3.0', 'files': files}))
        self.digest = sha(self.manifest)

    def verify(self):
        return bundle_check(self.root, self.digest, require_binary=False)

    def test_accepts_complete_official_sdk_inventory(self):
        self.assertEqual(len(self.verify()), hashlib.sha256().digest_size * 2)

    def test_next_release_version_uses_same_integrity_fences(self):
        data = json.loads(self.manifest.read_text())
        data['version'] = '0.3.3'
        self.manifest.write_text(json.dumps(data))
        self.digest = sha(self.manifest)
        self.assertEqual(len(self.verify()), 64)

    def test_invalid_release_version_is_rejected(self):
        for version in [None, 3, 'latest', '../0.3.3', '00.3.3']:
            data = json.loads(self.manifest.read_text())
            data['version'] = version
            self.manifest.write_text(json.dumps(data))
            self.digest = sha(self.manifest)
            with self.assertRaisesRegex(RuntimeError, 'manifest format'):
                self.verify()

    def test_source_mutation_is_rejected(self):
        (self.agent / 'src/main.mjs').write_text('changed')
        with self.assertRaisesRegex(RuntimeError, 'hash mismatch'):
            self.verify()

    def test_unlisted_runtime_file_is_rejected(self):
        (self.agent / 'src/unlisted.mjs').write_text('changed')
        with self.assertRaisesRegex(RuntimeError, 'inventory mismatch'):
            self.verify()

    def test_nonofficial_registry_is_rejected(self):
        next(iter(self.lock['packages'].values()))['resolved'] = 'https://untrusted.invalid/sdk.tgz'
        (self.agent / 'package-lock.json').write_text(json.dumps(self.lock))
        self.refresh()
        with self.assertRaisesRegex(RuntimeError, 'non-official registry'):
            self.verify()

    def test_missing_integrity_is_rejected(self):
        next(iter(self.lock['packages'].values())).pop('integrity')
        (self.agent / 'package-lock.json').write_text(json.dumps(self.lock))
        self.refresh()
        with self.assertRaisesRegex(RuntimeError, 'lacks integrity'):
            self.verify()

    def test_service_writable_directory_is_rejected(self):
        (self.agent / 'src').chmod(0o775)
        with self.assertRaisesRegex(RuntimeError, 'not service-writable'):
            self.verify()

    def test_symlink_source_is_rejected(self):
        main = self.agent / 'src/main.mjs'
        main.unlink()
        main.symlink_to(self.agent / 'test/fixture.test.mjs')
        self.refresh()
        with self.assertRaisesRegex(RuntimeError, 'symlink'):
            self.verify()

    def test_nested_service_writable_directory_is_rejected(self):
        nested = self.agent / 'src/nested'
        nested.mkdir(mode=0o775)
        nested.chmod(0o775)
        (nested / 'module.mjs').write_text('export {};')
        self.refresh()
        with self.assertRaisesRegex(RuntimeError, 'not service-writable'):
            self.verify()

    def test_npm_uses_distinct_empty_configuration_paths(self):
        env = npm_environment(self.root)
        self.assertNotEqual(env['NPM_CONFIG_GLOBALCONFIG'], env['NPM_CONFIG_USERCONFIG'])
        self.assertEqual(Path(env['NPM_CONFIG_GLOBALCONFIG']).read_bytes(), b'')
        self.assertEqual(env['NPM_CONFIG_REGISTRY'], 'https://registry.npmjs.org')

    def test_finished_unit_is_stopped_after_counters_are_preserved(self):
        process = Mock(returncode=None)
        process.poll.return_value = None
        process.wait.side_effect = lambda **_: setattr(process, 'returncode', 0)
        metrics = {'LoadState': 'loaded', 'SubState': 'exited', 'ExecMainStatus': '0', 'Result': 'success'}
        receipts = []
        with patch('release_bundle.subprocess.Popen', return_value=process), \
                patch('release_bundle.resource_snapshot', return_value=metrics), \
                patch('release_bundle.subprocess.run') as command:
            result = run_unit(['fake'], 'only-our-unit', self.root / 'unit.log', lambda p, v: receipts.append(v))
        self.assertEqual(result, metrics)
        self.assertEqual(receipts, [metrics])
        self.assertEqual(command.call_args_list[0].args[0], ['systemctl', 'stop', 'only-our-unit'])

    def test_failed_unit_never_yields_a_success_receipt(self):
        process = Mock(returncode=1)
        process.poll.return_value = 1
        metrics = {'LoadState': 'loaded', 'SubState': 'failed', 'ExecMainStatus': '1', 'Result': 'exit-code'}
        with patch('release_bundle.subprocess.Popen', return_value=process), \
                patch('release_bundle.resource_snapshot', return_value=metrics), \
                patch('release_bundle.subprocess.run'):
            with self.assertRaisesRegex(RuntimeError, 'candidate unit failed'):
                run_unit(['fake'], 'only-our-unit', self.root / 'unit.log', lambda p, v: None)


if __name__ == '__main__':
    unittest.main()
