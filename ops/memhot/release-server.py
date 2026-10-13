#!/usr/bin/env python3
"""Private, operator-invoked Memhot release stages; never emits credentials or content."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import sqlite3
import subprocess
import time
import urllib.error
import urllib.request
from release_bundle import bundle_verify, bundle_check, dependency_digest, node_check, sha

DATA = Path('/var/lib/memhot')
CURRENT = Path('/opt/memhot/current')
DROPIN = Path('/etc/systemd/system/memhot.service.d/20-invitation-beta.conf')
PROVIDER = DATA / 'providers.json'
ALPHAXIV = DATA / 'alphaxiv.json'
IDENTITY = pwd.getpwnam('memhot')


def private(path, owner=False):
    path.chmod(0o700 if path.is_dir() else 0o600)
    if owner:
        os.chown(path, IDENTITY.pw_uid, IDENTITY.pw_gid)


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')
    private(path)


def command(*args):
    # Captured diagnostic text stays out of stdout: configuration may be sensitive.
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'command failed: {args[0]} (exit {result.returncode})')
    return result.stdout.strip()


def backup(target):
    temp = target.with_suffix('.temporary')
    with sqlite3.connect(DATA.joinpath('memhot.sqlite').as_uri() + '?mode=ro', uri=True) as source:
        with sqlite3.connect(temp) as destination:
            source.backup(destination, pages=256, sleep=0.05)
            if destination.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
                raise RuntimeError('backup integrity failed')
    private(temp)
    temp.replace(target)


def inventory(path):
    with sqlite3.connect(path.as_uri() + '?mode=ro', uri=True) as conn:
        def digest(sql):
            rows = conn.execute(sql).fetchall()
            return hashlib.sha256(json.dumps(rows, ensure_ascii=False).encode()).hexdigest()
        return {
            'users': conn.execute('SELECT count(*) FROM users').fetchone()[0],
            'records': conn.execute('SELECT count(*) FROM records').fetchone()[0],
            'jobs': conn.execute('SELECT count(*) FROM jobs').fetchone()[0],
            'usersDigest': digest('SELECT id,name,profile FROM users ORDER BY id'),
            'recordsDigest': digest('SELECT user_id,kind,id,data,client_id,created FROM records ORDER BY user_id,kind,id'),
            'jobStates': conn.execute('SELECT status,count(*) FROM jobs GROUP BY status').fetchall(),
            'integrity': conn.execute('PRAGMA integrity_check').fetchone()[0],
        }


def copy_private(source, dest, owner=False):
    temp = dest.with_suffix(dest.suffix + '.incoming')
    shutil.copyfile(source, temp)
    private(temp, owner)
    temp.replace(dest)


def provider_check(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o777 != 0o600:
        raise RuntimeError('provider must be regular 0600 file')
    value = json.loads(path.read_text())
    for kind in ('text', 'asr'):
        config = value.get(kind)
        if not isinstance(config, dict) or not all(config.get(k) for k in ('apiKey', 'baseUrl', 'model')):
            raise RuntimeError('hosted provider incomplete')
        if not config['baseUrl'].startswith('https://'):
            raise RuntimeError('hosted provider must use HTTPS')


def prepare(stage):
    if (stage / 'prepared.json').exists():
        raise RuntimeError('stage already prepared; choose a fresh private directory')
    state = {
        'oldRelease': str(CURRENT.resolve(strict=True)),
        'hadDropin': DROPIN.exists(),
        'hadProvider': PROVIDER.exists(),
        'hadAlphaXiv': ALPHAXIV.exists(),
        'oldHealth': json.load(urllib.request.urlopen('http://127.0.0.1:8790/healthz', timeout=5)),
    }
    shutil.copyfile('/etc/systemd/system/memhot.service', stage / 'old-service.unit')
    private(stage / 'old-service.unit')
    for path, name in ((DROPIN, 'old-dropin.conf'), (PROVIDER, 'old-providers.json'), (ALPHAXIV, 'old-alphaxiv.json')):
        if path.exists():
            copy_private(path, stage / name)
    backup(stage / 'preliminary.sqlite')
    state['preliminaryInventory'] = inventory(stage / 'preliminary.sqlite')
    if PROVIDER.exists():
        copy_private(PROVIDER, stage / 'providers.json', owner=True)
    if ALPHAXIV.exists():
        copy_private(ALPHAXIV, stage / 'alphaxiv.json', owner=True)
    save(stage / 'prepared.json', state)
    print(json.dumps({'prepared': True, 'oldVersion': state['oldHealth']['version'],
                      'backupIntegrity': state['preliminaryInventory']['integrity']}))


def verify(stage, release, expected_sha, manifest_sha):
    state = json.loads((stage / 'prepared.json').read_text())
    binary = release / 'memhot-server'
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    if digest != expected_sha:
        raise RuntimeError('uploaded binary SHA256 mismatch')
    sdk = bundle_verify(stage, release, manifest_sha, save)
    provider_check(stage / 'providers.json')
    clone = stage / 'schema-check'
    if clone.exists():
        raise RuntimeError('schema clone already exists; do not overwrite evidence')
    clone.mkdir(mode=0o700)
    os.chown(clone, IDENTITY.pw_uid, IDENTITY.pw_gid)
    copy_private(stage / 'preliminary.sqlite', clone / 'memhot.sqlite', owner=True)
    alpha = stage / 'alphaxiv.json'
    extra = []
    if alpha.exists():
        if alpha.is_symlink() or alpha.stat().st_mode & 0o777 != 0o600:
            raise RuntimeError('paper config must be a regular 0600 file')
        owner = json.loads(alpha.read_text()).get('userId')
        with sqlite3.connect(clone / 'memhot.sqlite') as conn:
            if not conn.execute('SELECT 1 FROM users u LEFT JOIN accounts a ON a.user_id=u.id WHERE u.id=? AND (a.disabled=0 OR (a.user_id IS NULL AND EXISTS (SELECT 1 FROM account_invites i WHERE i.claim_user_id=u.id AND i.used=0 AND i.expires>?)))', (owner, int(time.time()))).fetchone():
                raise RuntimeError('paper credential must bind an active account or valid owner invitation')
        extra = [f'MEMHOT_ALPHAXIV_FILE={alpha}']
    command('runuser', '-u', 'memhot', '--', 'env', f'MEMHOT_DATA_DIR={clone}',
            f'MEMHOT_PROVIDER_FILE={stage / "providers.json"}', *extra, str(binary), 'check')
    after = inventory(clone / 'memhot.sqlite')
    before = state['preliminaryInventory']
    if any(after[k] != before[k] for k in ('users', 'records', 'usersDigest', 'recordsDigest')):
        raise RuntimeError('migration changed existing users, profiles or records')
    if after['integrity'] != 'ok':
        raise RuntimeError('migrated clone integrity failed')
    save(stage / 'verified.json', {'release': str(release), 'sha256': digest, 'manifestSha256': manifest_sha, 'sdkSourceDigest': sdk['sourceDigest'], 'dependenciesDigest': sdk['dependenciesDigest'], 'runtime': sdk['runtime'], 'dropinSha256': sha(stage / 'hosted-beta.conf'), 'providerSha256': sha(stage / 'providers.json'), 'alphaXivSha256': sha(alpha) if alpha.exists() else None, 'inventory': after})
    print(json.dumps({'schemaVerified': True, 'existingDataPreserved': True, 'sha256': digest}))


def atomic_link(release):
    temporary = CURRENT.with_name('current.incoming')
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(release)
    temporary.replace(CURRENT)


def health(expected):
    for _ in range(20):
        try:
            with urllib.request.urlopen('http://127.0.0.1:8790/healthz', timeout=3) as response:
                body = json.load(response)
            if body.get('status') == 'ok' and body.get('version') == expected:
                return body
        except (OSError, ValueError):
            pass
        time.sleep(1)
    raise RuntimeError('new version failed local health; rollback is available')


def activate(stage, expected_version):
    state = json.loads((stage / 'prepared.json').read_text())
    verified = json.loads((stage / 'verified.json').read_text())
    release = Path(verified['release'])
    if str(CURRENT.resolve()) != state['oldRelease']:
        raise RuntimeError('running release changed after preparation')
    if hashlib.sha256((release / 'memhot-server').read_bytes()).hexdigest() != verified['sha256']:
        raise RuntimeError('verified binary changed')
    bundle_check(release, verified['manifestSha256'])
    if node_check() != verified['runtime'] or dependency_digest(release / 'agent/node_modules') != verified['dependenciesDigest']:
        raise RuntimeError('verified SDK installation or runtime changed')
    if sha(stage / 'hosted-beta.conf') != verified['dropinSha256'] or sha(stage / 'providers.json') != verified['providerSha256']:
        raise RuntimeError('verified staged configuration changed')
    provider_check(stage / 'providers.json')
    alpha = stage / 'alphaxiv.json'
    if (sha(alpha) if alpha.exists() else None) != verified.get('alphaXivSha256'):
        raise RuntimeError('verified paper configuration changed')
    command('systemctl', 'stop', 'memhot')
    # Final stopped-boundary backup is the rollback source, not the earlier live copy.
    backup(stage / 'before-activation.sqlite')
    save(stage / 'before-activation.json', inventory(stage / 'before-activation.sqlite'))
    copy_private(stage / 'providers.json', PROVIDER, owner=True)
    if alpha.exists():
        copy_private(alpha, ALPHAXIV, owner=True)
    DROPIN.parent.mkdir(mode=0o755, exist_ok=True)
    copy_private(stage / 'hosted-beta.conf', DROPIN)
    DROPIN.chmod(0o644)
    atomic_link(release)
    command('systemctl', 'daemon-reload')
    command('systemctl', 'start', 'memhot')
    status = health(expected_version)
    save(stage / 'activated.json', {'health': status, 'release': str(release), 'sha256': verified['sha256']})
    print(json.dumps({'activated': True, 'version': status['version'], 'sha256': verified['sha256']}))


def rollback(stage):
    state = json.loads((stage / 'prepared.json').read_text())
    original = stage / 'before-activation.sqlite'
    if not original.exists():
        raise RuntimeError('no final pre-activation backup; refuse rollback')
    command('systemctl', 'stop', 'memhot')
    backup(stage / 'after-activation.sqlite')
    # Preserve post-upgrade state, including WAL files, for recovery rather than deleting it.
    for suffix in ('', '-wal', '-shm'):
        current = DATA / ('memhot.sqlite' + suffix)
        if current.exists():
            dest = stage / ('post-upgrade-original.sqlite' + suffix)
            if dest.exists():
                raise RuntimeError('rollback evidence already exists')
            current.replace(dest)
    copy_private(original, DATA / 'memhot.sqlite', owner=True)
    for path, present, name in ((DROPIN, state['hadDropin'], 'old-dropin.conf'),
                               (PROVIDER, state['hadProvider'], 'old-providers.json'),
                               (ALPHAXIV, state.get('hadAlphaXiv', False), 'old-alphaxiv.json')):
        if present:
            copy_private(stage / name, path, owner=path in (PROVIDER, ALPHAXIV))
        else:
            path.unlink(missing_ok=True)
    atomic_link(Path(state['oldRelease']))
    command('systemctl', 'daemon-reload')
    command('systemctl', 'start', 'memhot')
    status = health(state['oldHealth']['version'])
    save(stage / 'rolled-back.json', {'health': status, 'postUpgradeStatePreserved': True})
    print(json.dumps({'rolledBack': True, 'version': status['version']}))


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=('prepare', 'sdk-verify', 'verify', 'activate', 'rollback'))
    parser.add_argument('--stage', required=True, type=Path)
    parser.add_argument('--release', type=Path)
    parser.add_argument('--sha256')
    parser.add_argument('--manifest-sha256')
    parser.add_argument('--version', default='0.3.0')
    args = parser.parse_args()
    stage = args.stage.resolve()
    if os.geteuid() != 0 or not str(stage).startswith('/opt/memhot/deployments/'):
        raise SystemExit('requires root and dedicated private deployment directory')
    parent = Path('/opt/memhot/deployments')
    parent.mkdir(mode=0o710, exist_ok=True)
    parent.chmod(0o710)
    os.chown(parent, 0, IDENTITY.pw_gid)
    lock = (parent / 'release.lock').open('a')
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    stage.mkdir(mode=0o710, parents=True, exist_ok=True)
    stage.chmod(0o710)
    os.chown(stage, 0, IDENTITY.pw_gid)
    if args.action == 'prepare':
        prepare(stage)
    elif args.action == 'sdk-verify':
        if not args.release or not args.manifest_sha256:
            raise SystemExit('sdk-verify requires release and manifest SHA256')
        result = bundle_verify(stage, args.release.resolve(), args.manifest_sha256, save, require_binary=False)
        print(json.dumps({'sdkVerified': True, 'sourceDigest': result['sourceDigest'], 'runtime': result['runtime'], 'resourceProof': result['resourceProof']}))
    elif args.action == 'verify':
        if not args.release or not args.sha256 or not args.manifest_sha256:
            raise SystemExit('verify requires release, binary SHA256 and manifest SHA256')
        verify(stage, args.release.resolve(), args.sha256, args.manifest_sha256)
    elif args.action == 'activate':
        activate(stage, args.version)
    else:
        rollback(stage)


if __name__ == '__main__':
    main()
