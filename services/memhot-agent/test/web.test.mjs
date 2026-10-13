import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pdfFixture } from './fixtures/pdf.mjs';
import { PDF_LIMITS } from '../src/web-pdf.mjs';

async function run(start, respond, rawInput) {
  const workdir = await mkdtemp(join(tmpdir(), 'memhot-web-test-'));
  const child = spawn(process.execPath, ['--max-old-space-size=128', '--import', './test/fixtures/web-no-network.mjs', 'src/web-main.mjs'], {
    cwd: new URL('..', import.meta.url), env: { PATH: process.env.PATH, MEMHOT_WEB_WORKDIR: workdir,
      DEGOOG_API_KEY: 'sk-private-env-key', EXA_API_KEY: 'sk-private-exa-key', HTTP_PROXY: 'http://127.0.0.1:1' },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const frames = [];
  let fault, stderr = '';
  child.stderr.on('data', data => { stderr += data; });
  child.stdin.on('error', () => {});
  const lines = createInterface({ input: child.stdout });
  const timer = setTimeout(() => child.kill('SIGKILL'), 90_000);
  lines.on('line', async line => {
    try {
      const frame = JSON.parse(line);
      frames.push(frame);
      if (frame.type === 'http') {
        const answer = await respond(frame, workdir);
        child.stdin.write(`${JSON.stringify(answer.type === 'error' ? { ...answer, id: frame.id }
          : { type: 'result', id: frame.id, value: answer })}\n`);
      }
    } catch (error) { fault = error; child.kill(); }
  });
  child.stdin.write(rawInput ?? `${JSON.stringify(start)}\n`);
  const code = await new Promise(resolve => child.on('close', resolve));
  clearTimeout(timer); lines.close();
  const remaining = await readdir(workdir);
  await rm(workdir, { recursive: true, force: true });
  if (fault) throw fault;
  assert.equal(stderr, '', 'worker must not log plugin internals');
  assert.deepEqual(remaining, [], 'worker removes its isolated plugin config/cache');
  return { frames, code, terminal: frames.at(-1) };
}
const reply = (frame, body, contentType = 'application/json', status = 200) => ({ status, contentType, body, url: frame.url });

test('actual Pi plugin parses Degoog results without credentials, prefetch, or model calls', async () => {
  const result = await run({ type: 'start', action: 'search', provider: 'degoog', query: 'SQLite serverless' }, async (frame, workdir) => {
    assert.equal(frame.method, 'POST');
    assert.equal(frame.url, 'https://degoog.org/api/search');
    assert.deepEqual(JSON.parse(frame.body), { query: 'SQLite serverless', type: 'web', page: 1 });
    assert.deepEqual(Object.keys(frame.headers).sort(), ['accept', 'content-type']);
    const [configDir] = await readdir(workdir);
    const config = JSON.parse(await readFile(join(workdir, configDir, 'web-search.json'), 'utf8'));
    assert.deepEqual(config.webSearch.allowedProviders, ['degoog', 'searxng']);
    assert.equal(config.githubClone.enabled, false);
    assert.deepEqual(config.fetchRouting.providers, ['http']);
    return reply(frame, JSON.stringify({ results: [{ title: 'SQLite Is Serverless', url: 'https://sqlite.org/serverless.html',
      snippet: ' SQLite   reads database files directly. ' }] }));
  });
  assert.equal(result.code, 0, JSON.stringify(result.terminal));
  assert.deepEqual(result.terminal, { type: 'complete', sources: [{ title: 'SQLite Is Serverless',
    url: 'https://sqlite.org/serverless.html', text: 'SQLite reads database files directly.', fulltext: false }] });
  assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
  assert.doesNotMatch(JSON.stringify(result.frames), /sk-private|Authorization|api_key|summarize/);
});

test('actual Pi plugin uses the configured loopback SearXNG search API', async () => {
  const result = await run({ type: 'start', action: 'search', provider: 'searxng', endpoint: 'http://127.0.0.1:9876/search', query: '公开主题' }, frame => {
    const url = new URL(frame.url);
    assert.equal(frame.method, 'GET');
    assert.equal(url.origin, 'http://127.0.0.1:9876');
    assert.equal(url.pathname, '/search');
    assert.equal(url.searchParams.get('format'), 'json');
    assert.equal(url.searchParams.get('q'), '公开主题');
    return reply(frame, JSON.stringify({ results: [{ title: '官方资料', url: 'https://sqlite.org/docs.html', content: '检索摘要' }] }));
  });
  assert.equal(result.code, 0, JSON.stringify(result.terminal));
  assert.equal(result.terminal.sources[0].text, '检索摘要');
  assert.equal(result.terminal.sources[0].fulltext, false);
});

test('actual Pi readable extraction turns an official SQLite page fixture into source text', async () => {
  const article = 'With SQLite, the process that wants to access the database reads and writes directly from the database files on disk.';
  const html = `<!doctype html><html><head><title>SQLite Is Serverless</title></head><body><nav>Fixture navigation</nav>
    <main><article><h1>SQLite Is Serverless</h1>${Array.from({ length: 8 }, () => `<p>${article}</p>`).join('')}</article></main>
    <script>privateFixtureScript()</script></body></html>`;
  const result = await run({ type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, frame => {
    assert.equal(frame.method, 'GET');
    assert.equal(frame.url, 'https://sqlite.org/serverless.html');
    assert.ok(Object.keys(frame.headers).every(name => ['accept', 'content-type', 'user-agent'].includes(name)));
    return reply(frame, html, 'text/html; charset=utf-8');
  });
  assert.equal(result.code, 0, JSON.stringify(result.terminal));
  const [source] = result.terminal.sources;
  assert.equal(source.title, 'SQLite Is Serverless');
  assert.equal(source.fulltext, true);
  assert.ok(source.text.includes(article));
  assert.doesNotMatch(source.text, /<p>|privateFixtureScript|Fixture navigation/);
  assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
});

test('read source identity follows the host-approved redirect and raw GitHub text uses HTTP only', async () => {
  const initial = 'https://raw.githubusercontent.com/earendil-works/pi/main/README.md';
  const final = 'https://raw.githubusercontent.com/earendil-works/pi/main/docs/sdk.md';
  const text = '# Pi SDK fixture\n\n' + 'Native sessions manage repeated model calls and custom tools. '.repeat(20);
  const result = await run({ type: 'start', action: 'read', url: initial }, frame => {
    assert.equal(frame.url, initial);
    return { ...reply(frame, text, 'text/plain'), url: final };
  });
  assert.equal(result.code, 0, JSON.stringify(result.terminal));
  assert.equal(result.terminal.sources[0].url, final);
  assert.equal(result.terminal.sources[0].text, text);
  assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
});

test('HTTP denial and challenge pages stop without fallback or diagnostic leakage', async () => {
  for (const [status, body] of [[403, 'sk-private-provider-error'], [200, '<html><title>Just a moment...</title><div id="challenge-form">captcha</div></html>']]) {
    const result = await run({ type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, frame => reply(frame, body, 'text/html', status));
    assert.equal(result.terminal.type, 'failed');
    assert.equal(result.terminal.code, 'HTTP_DENIED');
    assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
    assert.doesNotMatch(JSON.stringify(result.frames), /sk-private|challenge-form|captcha/);
  }
});

test('worker rejects credential parameters, local/media/GitHub URLs, and untrusted search endpoints', async () => {
  for (const start of [
    { type: 'start', action: 'search', query: 'topic', provider: 'exa' },
    { type: 'start', action: 'search', query: 'topic', provider: 'degoog', apiKey: 'sk-private' },
    { type: 'start', action: 'search', query: 'topic', provider: 'searxng', endpoint: 'https://arbitrary.test' },
    ...['file:///etc/passwd', 'https://github.com/owner/repo', 'https://youtube.com/watch?v=abcdefghijk',
      'https://public.test/video.mp4', 'https://user:password@public.test/page', 'http://127.0.0.1/private'].map(url => ({ type: 'start', action: 'read', url })),
  ]) {
    const result = await run(start, () => assert.fail('invalid input reached HTTP'));
    assert.equal(result.terminal?.code, 'INVALID_START');
    assert.equal(result.frames.length, 1);
  }
});

test('HTTP body bounds and host failures end the plugin call without retries', async () => {
  for (const response of [frame => reply(frame, 'x'.repeat(2 * 1024 * 1024 + 1), 'text/plain'),
    () => ({ type: 'error', code: 'SCOPE_DENIED', message: 'sk-private-host-details' })]) {
    const result = await run({ type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, response);
    assert.equal(result.terminal.type, 'failed');
    assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
    assert.doesNotMatch(JSON.stringify(result.frames), /sk-private-host-details|node_modules|Error:/);
  }
});

test('web-specific input/output bounds reject oversized frames and accept a full two MiB document', async () => {
  const tooLarge = await run(null, () => assert.fail('oversized input reached HTTP'), `${'x'.repeat(12 * 1024 * 1024 + 1)}\n`);
  assert.equal(tooLarge.terminal.code, 'INPUT_LIMIT');
  assert.equal(tooLarge.frames.length, 1);
  const exact = await run({ type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, frame =>
    reply(frame, 'x'.repeat(2 * 1024 * 1024), 'text/plain'));
  assert.equal(exact.terminal.type, 'complete');
  assert.equal(exact.terminal.sources[0].text.length, 48_000);
  const escaped = await run({ type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, frame =>
    reply(frame, '\u0001'.repeat(48_000), 'text/plain'));
  assert.equal(escaped.terminal.code, 'OUTPUT_LIMIT');
  assert.ok(escaped.frames.every(frame => Buffer.byteLength(JSON.stringify(frame)) <= 256 * 1024));
});


test('public PDF reads bypass plugin routing, use host-approved bytes once, and expose extraction metadata', async () => {
  const start = { type: 'start', action: 'read', url: 'https://reports.example/report.pdf' };
  const result = await run(start, frame => ({ ...reply(frame, pdfFixture({ pages: 2, javascript: true }).toString('base64'), 'application/pdf'),
    bodyEncoding: 'base64', url: 'https://cdn.example/original.pdf' }));
  assert.equal(result.code, 0, JSON.stringify(result.terminal));
  assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
  const source = result.terminal.sources[0];
  assert.equal(source.url, 'https://cdn.example/original.pdf');
  assert.equal(source.fulltext, true);
  assert.deepEqual(source.extraction, { format: 'pdf', pagesRead: 2, totalPages: 2, truncated: false });
  assert.match(source.text, /public research/);
  assert.doesNotMatch(JSON.stringify(source), /2099|CreationDate|Publication|truncated=/);
});

test('an extensionless PDF and an eight MiB PDF fit the bounded host wire envelope', async () => {
  const data = pdfFixture({ padding: PDF_LIMITS.bytes - 2048 });
  assert.ok(data.length <= PDF_LIMITS.bytes && data.length > PDF_LIMITS.bytes - 2048);
  const result = await run({ type: 'start', action: 'read', url: 'https://reports.example/download?id=public' }, frame =>
    ({ ...reply(frame, data.toString('base64'), 'application/pdf'), bodyEncoding: 'base64' }));
  assert.equal(result.code, 0, JSON.stringify(result.terminal));
  assert.equal(result.terminal.sources[0].extraction.format, 'pdf');
});

test('PDF denial, corrupt signatures, missing encoding, and encrypted/scanned files do not trigger a retry', async () => {
  const pdf = pdfFixture().toString('base64');
  for (const response of [
    { status: 403, contentType: 'text/html', body: 'private provider error' },
    { status: 200, contentType: 'application/pdf', body: Buffer.from('<html>private challenge</html>').toString('base64'), bodyEncoding: 'base64' },
    { status: 200, contentType: 'application/pdf', body: pdf },
    { status: 200, contentType: 'application/pdf', body: pdfFixture({ encrypted: true }).toString('base64'), bodyEncoding: 'base64' },
    { status: 200, contentType: 'application/pdf', body: pdfFixture({ text: '' }).toString('base64'), bodyEncoding: 'base64' },
  ]) {
    const result = await run({ type: 'start', action: 'read', url: 'https://reports.example/report.pdf' }, frame => ({ ...response, url: frame.url }));
    assert.equal(result.terminal.type, 'failed');
    assert.equal(result.frames.filter(frame => frame.type === 'http').length, 1);
    assert.doesNotMatch(JSON.stringify(result.frames), /private provider|private challenge|Error:|node_modules/);
  }
});
