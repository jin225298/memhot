import test from 'node:test';
import assert from 'node:assert/strict';
import { chmod, lstat, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { CACHE_NAME, makeTransformManifest, prepareTransformCache, sha256 } from '../src/web-transform-cache.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'memhot-cache-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const sdkRoot = join(root, 'sdk'), template = join(sdkRoot, 'node_modules', '.cache', CACHE_NAME);
  for (const path of [template, join(sdkRoot, 'node_modules', 'jiti', 'dist'), join(sdkRoot, 'node_modules', 'pi-web-access')]) {
    await mkdir(path, { recursive: true, mode: 0o755 });
  }
  const source = join(sdkRoot, 'node_modules', 'pi-web-access', 'web-tool-core.ts');
  for (const [name, value] of [['package-lock.json', '{}'], ['node_modules/jiti/package.json', '{"version":"2.7.0"}'],
    ['node_modules/jiti/dist/jiti.cjs', 'module.exports = {};'], ['node_modules/pi-web-access/package.json', '{"version":"0.38.0"}'],
    ['node_modules/pi-web-access/web-tool-core.ts', 'export const staticCode: number = 1;']]) {
    await writeFile(join(sdkRoot, name), value, { mode: 0o644 });
  }
  const name = 'pi-web-access-web-tool-core.12345678.mjs', code = 'exports.staticCode = 1; /* v9-1234567890abcdef */\n';
  await writeFile(join(template, name), code, { mode: 0o644 });
  const manifest = await makeTransformManifest(sdkRoot, template);
  await writeFile(join(template, 'manifest.json'), JSON.stringify(manifest), { mode: 0o644 });
  const sandbox = async () => {
    const path = await mkdtemp(join(root, 'child-'));
    await chmod(path, 0o700);
    return path;
  };
  const prepare = async dir => prepareTransformCache(dir, { sdkRoot, ownerUid: process.getuid() });
  return { root, sdkRoot, template, source, name, code, manifest, sandbox, prepare };
}

test('verified static transforms get independent private writable copies without changing the template', async t => {
  const f = await fixture(t), before = sha256(await readFile(join(f.template, f.name)));
  const first = await f.prepare(await f.sandbox());
  assert.equal((await lstat(first)).mode & 0o777, 0o700);
  assert.equal((await lstat(join(first, f.name))).mode & 0o777, 0o600);
  assert.deepEqual(await readdir(first), [f.name]);
  await writeFile(join(first, f.name), 'private-query-and-response');
  await writeFile(join(first, 'child-only.mjs'), 'private runtime state');
  const second = await f.prepare(await f.sandbox());
  assert.notEqual(first, second);
  assert.equal(await readFile(join(second, f.name), 'utf8'), f.code);
  assert.deepEqual(await readdir(second), [f.name]);
  assert.equal(sha256(await readFile(join(f.template, f.name))), before);
  assert.doesNotMatch(JSON.stringify(f.manifest), /private-query|child-only/);
});

test('missing, modified, stale-source, and wrong-install templates fall back without partial copies', async t => {
  for (const mutate of [
    f => rm(f.template, { recursive: true }),
    f => rm(join(f.template, 'manifest.json')),
    f => writeFile(join(f.template, f.name), 'tampered compiled code'),
    f => writeFile(f.source, 'export const staticCode: number = 2;'),
    f => writeFile(join(f.sdkRoot, 'package-lock.json'), '{"changed":true}'),
    async f => { f.manifest.sdkRoot = '/different-release/agent'; await writeFile(join(f.template, 'manifest.json'), JSON.stringify(f.manifest)); },
  ]) {
    const f = await fixture(t), dir = await f.sandbox();
    await mutate(f);
    assert.equal(await f.prepare(dir), false);
    assert.deepEqual(await readdir(dir), []);
  }
});

test('unknown files, symlinks, unsafe permissions, and traversal names cannot become executable cache', async t => {
  for (const mutate of [
    f => writeFile(join(f.template, 'unexpected.mjs'), 'unlisted code'),
    async f => { await rm(join(f.template, f.name)); await symlink(f.source, join(f.template, f.name)); },
    f => chmod(join(f.template, f.name), 0o666),
    f => chmod(f.template, 0o777),
    f => symlink(f.root, join(f.sdkRoot, 'node_modules', 'pi-web-access', 'nested-source')),
    f => mkdir(join(f.sdkRoot, 'node_modules', 'pi-web-access', 'writable-source'), { mode: 0o777 }).then(() =>
      chmod(join(f.sdkRoot, 'node_modules', 'pi-web-access', 'writable-source'), 0o777)),
    async f => { f.manifest.files['../outside.mjs'] = f.manifest.files[f.name]; await writeFile(join(f.template, 'manifest.json'), JSON.stringify(f.manifest)); },
    async f => { f.manifest.sources['../outside.ts'] = f.manifest.sources['package-lock.json']; await writeFile(join(f.template, 'manifest.json'), JSON.stringify(f.manifest)); },
  ]) {
    const f = await fixture(t), dir = await f.sandbox();
    await mutate(f);
    assert.equal(await f.prepare(dir), false);
    assert.deepEqual(await readdir(dir), []);
  }
});

test('release seeding rejects another compiler/plugin version and unrecognized cache format', async t => {
  const f = await fixture(t);
  await writeFile(join(f.sdkRoot, 'node_modules', 'jiti', 'package.json'), '{"version":"3.0.0"}');
  await assert.rejects(makeTransformManifest(f.sdkRoot, f.template), /Unapproved/);
  await writeFile(join(f.sdkRoot, 'node_modules', 'jiti', 'package.json'), '{"version":"2.7.0"}');
  await rm(join(f.template, 'manifest.json'));
  await writeFile(join(f.template, f.name), 'code from an unrecognized transformer');
  await assert.rejects(makeTransformManifest(f.sdkRoot, f.template), /cache format/);
});
