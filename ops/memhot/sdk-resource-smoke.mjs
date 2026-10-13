// Synthetic RPC only: keep a real Pi session alive while its real web plugin reads fixture HTML.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(process.argv[2]);
const children = new Set();
let modelCalls = 0, toolCalls = 0, webCalls = 0;
const started = Date.now();
let sdkFirstFrameMs, webFirstFrameMs, webStarted;
function launch(script, env = {}, extra = []) {
  const child = spawn(process.execPath, ['--max-old-space-size=128', ...extra, script], {
    cwd: root, env, stdio: ['pipe', 'pipe', 'pipe'],
  });
  children.add(child);
  child.stdin.on('error', () => {});
  return child;
}
async function exchange(child, start, respond) {
  let fault, terminal, stderr = '';
  child.stderr.on('data', data => { stderr += data; });
  const lines = createInterface({ input: child.stdout });
  const pending = new Set();
  lines.on('line', line => {
    const work = (async () => {
      const frame = JSON.parse(line);
      if (['model', 'tool', 'http'].includes(frame.type)) {
        const value = await respond(frame);
        child.stdin.write(`${JSON.stringify({ type: 'result', id: frame.id, value })}\n`);
      } else if (['complete', 'failed'].includes(frame.type)) terminal = frame;
    })().catch(error => { fault = error; child.kill('SIGKILL'); });
    pending.add(work); work.finally(() => pending.delete(work));
  });
  child.stdin.write(`${JSON.stringify(start)}\n`);
  const code = await new Promise(resolve => child.once('close', resolve));
  await Promise.all(pending);
  children.delete(child); lines.close();
  if (fault) throw fault;
  assert.equal(code, 0, terminal?.code ?? 'child failed');
  assert.equal(stderr, '', 'unexpected child diagnostics');
  assert.equal(terminal?.type, 'complete');
  return terminal;
}
function sse(tool) {
  const delta = tool ? { role: 'assistant', tool_calls: [{ index: 0, id: 'fixture-tool', type: 'function',
    function: { name: 'read_source', arguments: '{"id":"fixture-source"}' } }] }
    : { role: 'assistant', content: '{"text":"Verified fixture evidence."}' };
  const chunk = (value, finish_reason) => ({ id: 'fixture-model', object: 'chat.completion.chunk', created: 1,
    model: 'test-model', choices: [{ index: 0, delta: value, finish_reason }] });
  return { contentType: 'text/event-stream', body: [chunk(delta, null), chunk({}, tool ? 'tool_calls' : 'stop'), '[DONE]']
    .map(value => `data: ${typeof value === 'string' ? value : JSON.stringify(value)}\n\n`).join('') };
}
const workdir = await mkdtemp(join(tmpdir(), 'memhot-combined-smoke-'));
const timer = setTimeout(() => { for (const child of children) child.kill('SIGKILL'); }, 90_000);
try {
  const sdk = launch('src/main.mjs');
  const result = await exchange(sdk, {
    type: 'start', runId: 'resource-fixture', mode: 'research',
    model: { protocol: 'openai', model: 'test-model' },
    prompt: 'Read the fixture and return the required JSON.', instructions: 'Use the read tool, then output JSON {"text":"answer"}.',
    tools: [{ name: 'read_source', description: 'Read the registered fixture source', inputSchema: {
      type: 'object', properties: { id: { type: 'string' } }, required: ['id'], additionalProperties: false,
    } }], maxSteps: 2, maxOutputTokens: 2000,
  }, async frame => {
    if (frame.type === 'model') { sdkFirstFrameMs ??= Date.now() - started; modelCalls++; return sse(modelCalls === 1); }
    assert.equal(frame.type, 'tool'); assert.equal(frame.name, 'read_source'); toolCalls++;
    webStarted = Date.now();
    const web = launch('src/web-main.mjs', { MEMHOT_WEB_WORKDIR: workdir, HOME: workdir },
      ['--import', './test/fixtures/web-no-network.mjs']);
    const article = 'SQLite reads and writes database files directly. This public fixture demonstrates an embedded database. ';
    const html = `<!doctype html><html><head><title>SQLite fixture</title></head><body><main><article><h1>SQLite fixture</h1>${
      Array.from({ length: 2000 }, () => `<p>${article}</p>`).join('')}</article></main></body></html>`;
    const source = await exchange(web, { type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, request => {
      assert.equal(request.type, 'http'); webFirstFrameMs ??= Date.now() - webStarted; webCalls++;
      return { status: 200, contentType: 'text/html', body: html, url: request.url };
    });
    assert.equal(source.sources[0].fulltext, true);
    assert.ok(source.sources[0].text.includes('SQLite'));
    return { materials: source.sources };
  });
  assert.equal(result.steps, 2); assert.equal(modelCalls, 2); assert.equal(toolCalls, 1); assert.equal(webCalls, 1);
  console.log(JSON.stringify({ passed: true, modelCalls, toolCalls, webCalls, paidCalls: 0,
    concurrentSdkAndWeb: true, sdkFirstFrameMs, webFirstFrameMs, elapsedMs: Date.now() - started }));
} finally {
  clearTimeout(timer);
  for (const child of children) child.kill('SIGKILL');
  await rm(workdir, { recursive: true, force: true });
}
