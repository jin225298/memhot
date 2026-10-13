// Deployment-only free public capability check; fixed generic query and official source.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(process.argv[2]);
const page = 'https://sqlite.org/serverless.html';
const workdir = await mkdtemp(join(tmpdir(), 'memhot-public-smoke-'));
async function run(start) {
  const began = Date.now();
  const child = spawn(process.execPath, ['--max-old-space-size=128', 'src/web-main.mjs'], {
    cwd: root, env: { MEMHOT_WEB_WORKDIR: workdir, HOME: workdir }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  const lines = createInterface({ input: child.stdout });
  let terminal, fault, stderr = '', requests = 0;
  child.stderr.on('data', data => { stderr += data; });
  child.stdin.on('error', () => {});
  const timer = setTimeout(() => child.kill('SIGKILL'), 90_000);
  const pending = new Set();
  lines.on('line', line => {
    const task = (async () => {
      const frame = JSON.parse(line);
      if (frame.type !== 'http') { terminal = frame; return; }
      requests++;
      assert.equal(frame.url, start.action === 'search' ? 'https://degoog.org/api/search' : page);
      assert.equal(frame.method, start.action === 'search' ? 'POST' : 'GET');
      const response = await fetch(frame.url, {
        method: frame.method, headers: { ...frame.headers, 'user-agent': 'Memhot/0.3 public deployment check' },
        ...(frame.body ? { body: frame.body } : {}), redirect: 'error', signal: AbortSignal.timeout(15_000),
      });
      const chunks = []; let size = 0;
      for await (const chunk of response.body) {
        size += chunk.length; assert.ok(size <= 2 * 1024 * 1024, 'public HTTP body limit'); chunks.push(chunk);
      }
      child.stdin.write(`${JSON.stringify({ type: 'result', id: frame.id, value: {
        status: response.status, contentType: response.headers.get('content-type') ?? 'text/plain',
        url: response.url, body: Buffer.concat(chunks).toString('utf8'),
      } })}\n`);
    })().catch(error => { fault = error; child.kill('SIGKILL'); });
    pending.add(task); task.finally(() => pending.delete(task));
  });
  child.stdin.write(`${JSON.stringify(start)}\n`);
  const code = await new Promise(resolve => child.once('close', resolve));
  clearTimeout(timer); lines.close(); await Promise.all(pending);
  if (fault) throw fault;
  assert.equal(code, 0, terminal?.code ?? 'plugin child failed');
  assert.equal(stderr, ''); assert.equal(terminal?.type, 'complete'); assert.equal(requests, 1);
  assert.ok(terminal.sources.length > 0);
  if (start.action === 'read') {
    assert.equal(terminal.sources[0].url, page); assert.equal(terminal.sources[0].fulltext, true);
    assert.ok(terminal.sources[0].text.includes('SQLite') && terminal.sources[0].text.length > 500);
  }
  return { action: start.action, passed: true, elapsedMs: Date.now() - began,
    requests, sourceCount: terminal.sources.length,
    ...(start.action === 'read' ? { url: page, fulltextChars: terminal.sources[0].text.length } : {}) };
}
try {
  const search = await run({ type: 'start', action: 'search', provider: 'degoog', query: 'SQLite embedded database official documentation' });
  const read = await run({ type: 'start', action: 'read', url: page });
  console.log(JSON.stringify({ publicCapability: 'passed', modelCalls: 0, paidCalls: 0, search, read }));
} finally { await rm(workdir, { recursive: true, force: true }); }
