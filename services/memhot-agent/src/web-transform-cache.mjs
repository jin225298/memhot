import { createHash } from 'node:crypto';
import { chmod, lstat, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const CACHE_NAME = 'memhot-web-jiti-v1';
const SDK_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const HASH = /^[a-f0-9]{64}$/;
const CACHE_FILE = /^[\w.-]{1,200}\.(?:mjs|cjs)$/;
const MAX_FILES = 256, MAX_BYTES = 8 * 1024 * 1024;
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

// The release manifest/dependency digest protects this root-owned template. Its
// own manifest additionally binds compiled code to this install path and source.
async function immutable(path, ownerUid, directory = false) {
  const stat = await lstat(path);
  if (stat.isSymbolicLink() || stat.uid !== ownerUid || (stat.mode & 0o022) ||
    (directory ? !stat.isDirectory() : !stat.isFile())) throw new Error('Untrusted transform template');
  return stat;
}

export async function sourceInventory(sdkRoot, ownerUid) {
  const files = ['package-lock.json', 'node_modules/jiti/package.json',
    'node_modules/jiti/dist/jiti.cjs', 'node_modules/pi-web-access/package.json'];
  async function visit(relative) {
    if (ownerUid !== undefined) await immutable(join(sdkRoot, relative), ownerUid, true);
    for (const entry of await readdir(join(sdkRoot, relative), { withFileTypes: true })) {
      if (entry.isSymbolicLink()) throw new Error('Source symlinks are unavailable');
      const name = `${relative}/${entry.name}`;
      if (entry.isDirectory()) await visit(name);
      else if (entry.isFile() && name.endsWith('.ts')) files.push(name);
    }
  }
  await visit('node_modules/pi-web-access');
  return files.sort();
}

export async function makeTransformManifest(sdkRoot, template) {
  for (const [name, expected] of [['jiti', '2.7.0'], ['pi-web-access', '0.38.0']]) {
    const installed = JSON.parse(await readFile(join(sdkRoot, 'node_modules', name, 'package.json'), 'utf8'));
    if (installed.version !== expected) throw new Error('Unapproved transform dependency');
  }
  const sources = {}, files = {};
  for (const name of await sourceInventory(sdkRoot)) sources[name] = sha256(await readFile(join(sdkRoot, name)));
  for (const name of (await readdir(template)).sort()) {
    if (!CACHE_FILE.test(name)) throw new Error('Unexpected transform file');
    const bytes = await readFile(join(template, name));
    if (!/\/\* v9-[a-f0-9]{16} \*\/\n$/.test(bytes.toString())) throw new Error('Unexpected Jiti cache format');
    files[name] = sha256(bytes);
  }
  return { schema: 1, sdkRoot: await realpath(sdkRoot), node: process.version,
    jiti: '2.7.0', plugin: '0.38.0', sources, files };
}

/** A new writable copy for one child only; absence or mismatch uses cold import. */
export async function prepareTransformCache(sandboxDir, { sdkRoot = SDK_ROOT,
  ownerUid = process.platform === 'linux' ? 0 : process.getuid() } = {}) {
  const target = join(sandboxDir, 'jiti-cache');
  try {
    sdkRoot = await realpath(sdkRoot);
    const template = join(sdkRoot, 'node_modules', '.cache', CACHE_NAME);
    for (const path of [sdkRoot, join(sdkRoot, 'node_modules'), join(sdkRoot, 'node_modules', '.cache'), template,
      join(sdkRoot, 'node_modules', 'pi-web-access'), join(sdkRoot, 'node_modules', 'jiti'),
      join(sdkRoot, 'node_modules', 'jiti', 'dist')]) {
      await immutable(path, ownerUid, true);
    }
    const manifestPath = join(template, 'manifest.json');
    if ((await immutable(manifestPath, ownerUid)).size > 128 * 1024) return false;
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
    if (manifest.schema !== 1 || manifest.sdkRoot !== sdkRoot || manifest.node !== process.version ||
      manifest.jiti !== '2.7.0' || manifest.plugin !== '0.38.0' || !manifest.sources || !manifest.files) return false;
    const sources = await sourceInventory(sdkRoot, ownerUid);
    if (JSON.stringify(sources) !== JSON.stringify(Object.keys(manifest.sources).sort())) return false;
    let sourceBytes = 0;
    for (const name of sources) {
      if (!HASH.test(manifest.sources[name])) return false;
      const source = join(sdkRoot, name);
      const stat = await immutable(source, ownerUid);
      if ((sourceBytes += stat.size) > 16 * 1024 * 1024) return false;
      if (sha256(await readFile(source)) !== manifest.sources[name]) return false;
    }
    const names = Object.keys(manifest.files).sort();
    if (!names.length || names.length > MAX_FILES ||
      JSON.stringify([...names, 'manifest.json'].sort()) !== JSON.stringify((await readdir(template)).sort())) return false;
    const copies = [];
    let total = 0;
    for (const name of names) {
      if (!CACHE_FILE.test(name) || !HASH.test(manifest.files[name])) return false;
      const source = join(template, name), stat = await immutable(source, ownerUid);
      if (stat.size > 2 * 1024 * 1024 || (total += stat.size) > MAX_BYTES) return false;
      const bytes = await readFile(source);
      if (sha256(bytes) !== manifest.files[name]) return false;
      copies.push([name, bytes]);
    }
    await mkdir(target, { mode: 0o700 });
    await chmod(target, 0o700);
    for (const [name, bytes] of copies) await writeFile(join(target, name), bytes, { mode: 0o600, flag: 'wx' });
    return target;
  } catch {
    // Never trust a partial/stale cache, and never write into the release tree.
    await rm(target, { recursive: true, force: true }).catch(() => {});
    return false;
  }
}
