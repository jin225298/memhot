// Release preparation only: actual plugin transforms, fixed offline fixtures,
// no model, DNS, HTTP, credentials, query history, or persisted plugin config.
import { chmod, lstat, mkdtemp, readdir, realpath, rename, rm, writeFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import dns from 'node:dns/promises';
import { syncBuiltinESMExports } from 'node:module';
import { CACHE_NAME, makeTransformManifest, sha256 } from './web-transform-cache.mjs';

const began = performance.now();
const sdk = await realpath(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
if (process.argv.length !== 3 || await realpath(process.argv[2]) !== sdk ||
  (process.platform === 'linux' && process.getuid() !== 0)) throw new Error('Run root release seeding against the final SDK path');
const parent = join(sdk, 'node_modules', '.cache'), target = join(parent, CACHE_NAME);
await mkdir(parent, { recursive: true, mode: 0o755 });
await chmod(parent, 0o755);
try { await lstat(target); throw new Error('Transform template already exists; regenerate after npm ci'); }
catch (error) { if (error.code !== 'ENOENT') throw error; }
const cache = await mkdtemp(join(parent, '.web-jiti-build-'));
const home = await mkdtemp(join(tmpdir(), 'memhot-web-seed-'));
try {
  for (const name of Object.keys(process.env)) delete process.env[name];
  process.env.PI_CODING_AGENT_DIR = home;
  dns.lookup = async () => [{ address: '93.184.216.34', family: 4 }];
  syncBuiltinESMExports();
  let requests = 0;
  globalThis.fetch = async input => {
    requests++;
    const url = String(input instanceof Request ? input.url : input);
    if (url === 'https://degoog.org/api/search') return new Response(JSON.stringify({ results: [{ title: 'Static seed fixture',
      url: 'https://sqlite.org/serverless.html', snippet: 'Offline compiler warmup fixture.' }] }),
    { headers: { 'content-type': 'application/json' } });
    if (url === 'https://sqlite.org/serverless.html') return new Response('<!doctype html><html><head><title>Static seed fixture</title></head><body><main><article>' +
      '<p>Offline compiler warmup checks readable extraction without storing a webpage result.</p>'.repeat(15) +
      '</article></main></body></html>', { headers: { 'content-type': 'text/html' } });
    throw new Error('Release compiler warmup forbids network requests');
  };
  const config = { searchProvider: 'degoog', workflow: 'standard', maxInlineContentChars: 48_000,
    webSearch: { allowedProviders: ['degoog', 'searxng'] }, fetch: { defaultMode: 'readable', allowedModes: ['readable'] },
    fetchRouting: { providers: ['http'], allowRemoteHostedProviders: false },
    githubClone: { enabled: false }, githubPrIssue: { enabled: false }, youtube: { enabled: false },
    video: { enabled: false }, pdf: { enabled: false }, image: { enabled: false } };
  await writeFile(join(home, 'web-search.json'), JSON.stringify(config), { mode: 0o600, flag: 'wx' });
  const { createJiti } = await import('jiti');
  const jiti = createJiti(new URL('./web-worker.mjs', import.meta.url).href, { fsCache: cache, moduleCache: true });
  const { createStandaloneWebToolCore } = await jiti.import(import.meta.resolve('pi-web-access/web-tool-core.ts'));
  const core = createStandaloneWebToolCore();
  const search = await core.webSearch({ queries: ['memhot-static-compiler-seed'], provider: 'degoog', numResults: 1, includeContent: false });
  const read = await core.fetchContent({ url: 'https://sqlite.org/serverless.html', mode: 'readable' });
  if (search.isError || read.isError || requests !== 2) throw new Error('Offline plugin warmup did not complete');
  const manifest = await makeTransformManifest(sdk, cache);
  const encoded = JSON.stringify(manifest) + '\n';
  await writeFile(join(cache, 'manifest.json'), encoded, { mode: 0o644, flag: 'wx' });
  let bytes = 0;
  for (const name of await readdir(cache)) {
    const path = join(cache, name), stat = await lstat(path);
    if (!stat.isFile() || stat.isSymbolicLink()) throw new Error('Unexpected compiler cache entry');
    bytes += stat.size;
    await chmod(path, 0o644);
  }
  await chmod(cache, 0o755);
  await rename(cache, target);
  process.stdout.write(JSON.stringify({ passed: true, files: Object.keys(manifest.files).length, bytes,
    manifestSha256: sha256(encoded), elapsedMs: Math.round(performance.now() - began), networkCalls: 0, modelCalls: 0 }) + '\n');
} finally {
  await rm(cache, { recursive: true, force: true });
  await rm(home, { recursive: true, force: true });
}
