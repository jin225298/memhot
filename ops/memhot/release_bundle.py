"""Release integrity and credential-free Linux Pi smoke checks; no activation here."""
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import subprocess
import tarfile
import time

NODE_VERSION = 'v22.23.3'
NODE_ROOT = Path('/opt/memhot/runtime/node-v22.23.3-linux-x64')
NODE = NODE_ROOT / 'bin/node'
NODE_ARCHIVE = NODE_ROOT.parent / (NODE_ROOT.name + '.tar.xz')
NODE_ARCHIVE_SHA = 'df450af89261115ef9f9e3830c3eeb2cc9213b63c720b1af623cb5dcbe2e02de'
SDK_PINS = {'@earendil-works/pi-coding-agent': '1.1.0', '@earendil-works/pi-ai': '1.1.0',
            '@earendil-works/pi-mcp': '1.1.0',
            'pi-web-access': '0.38.0', 'jiti': '2.7.0'}


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def node_check():
    if sha(NODE_ARCHIVE) != NODE_ARCHIVE_SHA:
        raise RuntimeError('official Node archive SHA256 mismatch')
    with tarfile.open(NODE_ARCHIVE, 'r:xz') as archive:
        source = archive.extractfile(f'{NODE_ROOT.name}/bin/node')
        digest = hashlib.sha256()
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    actual = sha(NODE)
    if digest.hexdigest() != actual:
        raise RuntimeError('isolated Node binary differs from verified archive')
    version = subprocess.run([str(NODE), '--version'], env={}, capture_output=True, text=True, check=True).stdout.strip()
    if version != NODE_VERSION:
        raise RuntimeError('isolated Node version mismatch')
    return {'version': version, 'binarySha256': actual, 'archiveSha256': NODE_ARCHIVE_SHA}


def source_inventory(release):
    agent = release / 'agent'
    required = {'agent/package.json', 'agent/package-lock.json', 'agent/src/main.mjs'}
    actual = {'agent/package.json', 'agent/package-lock.json'}
    if (agent / '.gitignore').exists():
        actual.add('agent/.gitignore')
    for name in ('src', 'test'):
        actual.update(path.relative_to(release).as_posix() for path in (agent / name).rglob('*') if path.is_file() or path.is_symlink())
    if not required.issubset(actual) or not any(name.startswith('agent/test/') for name in actual):
        raise RuntimeError('agent source or test inventory incomplete')
    return actual


def bundle_check(release, expected_manifest_sha, require_binary=True):
    manifest = release / 'release-manifest.json'
    for path in (release, manifest, release / 'agent', release / 'agent/src', release / 'agent/test'):
        if path.is_symlink() or path.stat().st_uid != 0 or path.stat().st_mode & 0o022:
            raise RuntimeError('release paths must be root-owned and not service-writable')
    if sha(manifest) != expected_manifest_sha:
        raise RuntimeError('release manifest SHA256 mismatch')
    document = json.loads(manifest.read_text())
    files = document.get('files')
    if not isinstance(document.get('version'), str) or not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', document['version']) or not isinstance(files, dict):
        raise RuntimeError('release manifest format invalid')
    expected = source_inventory(release)
    if require_binary:
        expected.add('memhot-server')
    if set(files) != expected:
        raise RuntimeError('release manifest file inventory mismatch')
    for name, digest in files.items():
        path = release / name
        relative = Path(name)
        if relative.is_absolute() or '..' in relative.parts:
            raise RuntimeError('release manifest path invalid')
        for parent in [path, *path.parents]:
            if parent == release.parent:
                break
            if parent.is_symlink():
                raise RuntimeError('release source path is a symlink')
            if parent.stat().st_uid != 0 or parent.stat().st_mode & 0o022:
                raise RuntimeError('release source paths must be root-owned and not service-writable')
        if not path.is_file() or sha(path) != digest:
            raise RuntimeError(f'release file hash mismatch: {name}')
        if path.stat().st_uid != 0 or path.stat().st_mode & 0o022:
            raise RuntimeError('release source must be root-owned and not writable by the service')
    package = json.loads((release / 'agent/package.json').read_text())
    locked = json.loads((release / 'agent/package-lock.json').read_text())
    for package_name, version in SDK_PINS.items():
        if package['dependencies'].get(package_name) != version or locked['packages'][f'node_modules/{package_name}']['version'] != version:
            raise RuntimeError('approved SDK/plugin version mismatch')
    for item in locked['packages'].values():
        resolved = item.get('resolved', '')
        if resolved and not resolved.startswith('https://registry.npmjs.org/'):
            raise RuntimeError('SDK lockfile contains a non-official registry source')
        if resolved and not item.get('integrity'):
            raise RuntimeError('SDK lockfile package lacks integrity')
    source_files = sorted((name, digest) for name, digest in files.items() if name.startswith('agent/'))
    return hashlib.sha256(json.dumps(source_files, separators=(',', ':')).encode()).hexdigest()


def dependency_digest(path):
    if not path.is_dir() or path.is_symlink() or path.stat().st_uid != 0 or path.stat().st_mode & 0o022:
        raise RuntimeError('SDK dependency root must be root-owned and not service-writable')
    digest = hashlib.sha256()
    for item in sorted(path.rglob('*')):
        name = item.relative_to(path).as_posix()
        if item.lstat().st_uid != 0 or (not item.is_symlink() and item.lstat().st_mode & 0o022):
            raise RuntimeError('SDK dependency paths must be root-owned and not service-writable')
        if item.is_symlink():
            if not item.resolve().is_relative_to(path.resolve()):
                raise RuntimeError('SDK dependency symlink escapes installation')
            value = 'link:' + os.readlink(item)
        elif item.is_file():
            if item.stat().st_uid != 0 or item.stat().st_mode & 0o022:
                raise RuntimeError('SDK dependencies must be root-owned and not service-writable')
            value = sha(item)
        else:
            continue
        digest.update(f'{name}\0{value}\n'.encode())
    return digest.hexdigest()


def run_private(args, evidence, env=None, cwd=None, umask=0o077):
    with evidence.open('wb') as output:
        result = subprocess.run(args, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT, umask=umask, timeout=600)
    evidence.chmod(0o600)
    if result.returncode:
        raise RuntimeError(f'candidate command failed; private evidence: {evidence.name}')


def resource_snapshot(unit):
    result = subprocess.run(['systemctl', 'show', unit, '-p', 'LoadState', '-p', 'SubState',
                             '-p', 'ExecMainStatus', '-p', 'ControlGroup', '-p', 'MemoryPeak',
                             '-p', 'CPUUsageNSec', '-p', 'CPUQuotaPerSecUSec', '-p', 'Result', '-p', 'TasksMax', '-p', 'MemoryMax'],
                            capture_output=True, text=True, check=True).stdout.strip()
    metrics = dict(line.split('=', 1) for line in result.splitlines() if '=' in line)
    if metrics.get('LoadState') != 'loaded':
        return {}
    group = metrics.get('ControlGroup', '')
    if group.startswith('/'):
        base = Path('/sys/fs/cgroup') / group.lstrip('/')
        for name in ('memory.peak', 'memory.swap.peak', 'memory.events', 'pids.peak'):
            path = base / name
            if path.exists():
                metrics[name] = path.read_text().strip()
    return metrics


def run_unit(args, unit, evidence, save):
    metrics = {}
    started = time.monotonic()
    with evidence.open('wb') as output:
        process = subprocess.Popen(args, stdout=output, stderr=subprocess.STDOUT)
        try:
            while process.poll() is None:
                current = resource_snapshot(unit)
                metrics.update({key: value for key, value in current.items() if value})
                # --remain-after-exit preserves cgroup peak counters; --wait needs
                # an explicit stop after the actual command has finished.
                if current.get('SubState') in ('exited', 'failed'):
                    break
                if time.monotonic() - started > 1800:
                    raise RuntimeError('candidate unit exceeded the verification deadline')
                time.sleep(0.5)
        finally:
            try:
                metrics.update({key: value for key, value in resource_snapshot(unit).items() if value})
                save(evidence.with_suffix('.resources.json'), metrics)
            finally:
                subprocess.run(['systemctl', 'stop', unit], capture_output=True)
                subprocess.run(['systemctl', 'reset-failed', unit], capture_output=True)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    evidence.chmod(0o600)
    if process.returncode or metrics.get('ExecMainStatus') != '0' or metrics.get('Result') != 'success':
        raise RuntimeError(f'candidate unit failed; private evidence: {evidence.name}')
    return metrics


def npm_environment(stage):
    config = stage / 'npm-empty-global.conf'
    if config.is_symlink() or (config.exists() and config.read_bytes()):
        raise RuntimeError('isolated npm configuration must be empty')
    config.touch(mode=0o600, exist_ok=True)
    config.chmod(0o600)
    return {'PATH': f'{NODE_ROOT / "bin"}:/usr/bin:/bin', 'HOME': str(stage),
            'NPM_CONFIG_REGISTRY': 'https://registry.npmjs.org', 'NPM_CONFIG_USERCONFIG': '/dev/null',
            'NPM_CONFIG_GLOBALCONFIG': str(config), 'NPM_CONFIG_CACHE': str(stage / 'npm-cache'),
            'NPM_CONFIG_AUDIT': 'false', 'NPM_CONFIG_FUND': 'false'}


def seed_transform_cache(stage, release, attempt, save):
    sdk = release / 'agent'
    parent = sdk / 'node_modules/.cache'
    if parent.is_symlink():
        raise RuntimeError('transform cache parent cannot be a symlink')
    parent.mkdir(mode=0o755, exist_ok=True)
    parent.chmod(0o755)
    unit = f'memhot-web-cache-seed-{time.time_ns()}'
    args = ['systemd-run', '--quiet', '--wait', '--pipe', '--remain-after-exit', f'--unit={unit}',
            '-p', 'User=root', '-p', f'WorkingDirectory={sdk}', '-p', 'MemoryHigh=256M',
            '-p', 'MemoryMax=384M', '-p', 'MemoryAccounting=yes', '-p', 'CPUQuota=100%',
            '-p', 'CPUAccounting=yes', '-p', 'TasksMax=64', '-p', 'ProtectHome=true',
            '-p', 'ProtectSystem=strict', '-p', 'NoNewPrivileges=true', '-p', 'PrivateTmp=true',
            '-p', 'PrivateDevices=true', '-p', 'IPAddressDeny=any', '-p', f'ReadWritePaths={parent}',
            '/usr/bin/env', '-i', str(NODE), '--max-old-space-size=128',
            str(sdk / 'src/seed-web-cache.mjs'), str(sdk)]
    evidence = attempt / 'web-transform-seed.log'
    metrics = run_unit(args, unit, evidence, save)
    result = json.loads(next(line for line in evidence.read_text().splitlines() if line.startswith('{')))
    manifest = parent / 'memhot-web-jiti-v1/manifest.json'
    if result.get('passed') is not True or result.get('networkCalls') != 0 or result.get('modelCalls') != 0 or sha(manifest) != result.get('manifestSha256'):
        raise RuntimeError('offline web transform seed verification failed')
    return {'manifestSha256': sha(manifest), 'files': result['files'], 'bytes': result['bytes'],
            'resourceProof': metrics, 'logSha256': sha(evidence)}


def bundle_verify(stage, release, manifest_sha, save, require_binary=True):
    source_digest = bundle_check(release, manifest_sha, require_binary)
    runtime = node_check()
    receipt = stage / 'sdk-verified.json'
    if receipt.exists():
        previous = json.loads(receipt.read_text())
        if previous.get('release') == str(release) and previous.get('sourceDigest') == source_digest and previous.get('runtime') == runtime and previous.get('dependenciesDigest') == dependency_digest(release / 'agent/node_modules'):
            # Source/dependency hashes alone do not make a copied cache reusable:
            # Jiti also binds compiled files to their absolute installation path.
            template = release / 'agent/node_modules/.cache/memhot-web-jiti-v1/manifest.json'
            if (not template.is_file()
                    or json.loads(template.read_text()).get('sdkRoot') != str((release / 'agent').resolve())
                    or sha(template) != previous.get('transformCache', {}).get('manifestSha256')):
                raise RuntimeError('SDK transform cache belongs to another release; reseed and verify in a fresh stage')
            return previous
        raise RuntimeError('SDK verification evidence changed; use a fresh private stage')
    home = stage / 'sdk-home'
    home.mkdir(mode=0o700, exist_ok=True)
    attempt = stage / f'sdk-check-{time.time_ns()}'
    attempt.mkdir(mode=0o700)
    service = pwd.getpwnam('memhot')
    os.chown(home, service.pw_uid, service.pw_gid)
    env = npm_environment(stage)
    npm = NODE_ROOT / 'lib/node_modules/npm/bin/npm-cli.js'
    run_private([str(NODE), '--max-old-space-size=128', str(npm), 'ci', '--ignore-scripts', '--include=optional', '--registry=https://registry.npmjs.org'], attempt / 'sdk-npm-ci.log', env, release / 'agent', umask=0o022)
    transform = seed_transform_cache(stage, release, attempt, save)
    unit = f'memhot-sdk-verify-{time.time_ns()}'
    tests = sorted(str(path) for path in (release / 'agent/test').glob('*.test.mjs'))
    args = ['systemd-run', '--quiet', '--wait', '--pipe', '--remain-after-exit', f'--unit={unit}',
            '-p', 'User=memhot', '-p', f'WorkingDirectory={release / "agent"}',
            '-p', 'MemoryHigh=256M', '-p', 'MemoryMax=384M', '-p', 'MemoryAccounting=yes',
            '-p', 'CPUQuota=100%', '-p', 'CPUAccounting=yes', '-p', 'TasksMax=64',
            '-p', 'ProtectHome=true', '-p', 'ProtectSystem=strict', '-p', 'NoNewPrivileges=true',
            '-p', 'PrivateTmp=true', '-p', 'PrivateDevices=true', '-p', 'IPAddressDeny=any',
            '-p', f'ReadWritePaths={home}', '/usr/bin/env', '-i', f'PATH={env["PATH"]}', f'HOME={home}',
            str(NODE), '--max-old-space-size=128', '--test', '--test-concurrency=1', *tests]
    metrics = run_unit(args, unit, attempt / 'sdk-linux-tests.log', save)
    result = {'release': str(release), 'sourceDigest': source_digest, 'runtime': runtime,
              'dependenciesDigest': dependency_digest(release / 'agent/node_modules'),
              'linuxTests': 'passed', 'resourceProof': metrics, 'transformCache': transform,
              'evidenceDirectory': str(attempt), 'testLogSha256': sha(attempt / 'sdk-linux-tests.log')}
    save(receipt, result)
    return result
