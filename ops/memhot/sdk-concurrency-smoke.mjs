// Offline resource probe: four real Pi sessions plus one real shared web worker.
// Usage: node ops/memhot/sdk-concurrency-smoke.mjs services/memhot-agent
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

assert.ok(process.argv[2], 'Pass the services/memhot-agent directory.');
const root = resolve(process.argv[2]);
const started = Date.now();
const elapsed = () => Date.now() - started;
const children = new Set(), allChildren = [], exchanges = [];
const lanes = ['chat', 'translation', 'digest', 'research'];
const firstFrameMs = {}, firstModelFrameMs = {}, modelCallsByLane = {}, toolCallsByLane = {};
let failure, completed = 0, webWorkers = 0, webCalls = 0, deniedNetworkAttempts = 0;
let peakSdkChildren = 0, peakWebChildren = 0, peakChildren = 0;
let timeToAllReadyMs, webStartedMs, webFirstFrameMs, webCompletedMs;
let sdkChildrenAtWebStart, sdkChildrenAtWebHttp, sdkChildrenAtWebComplete;
let sampledProcessRssPeakBytes = null, sampledCgroupMemoryPeakBytes = null;
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  promise.catch(() => {});
  return { promise, resolve, reject };
};
const aborted = deferred(), allReady = deferred(), allReaders = deferred(), webDone = deferred();
const ready = new Set(), readers = new Set();
const wait = promise => Promise.race([promise, aborted.promise]);
function fail(error) {
  if (!failure) { failure = error; aborted.reject(error); }
  killChildren();
}
function killChildren() {
  for (const { child } of children) {
    // Failed spawns have no PID; never signal an uninitialised process handle.
    if (child.pid && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  }
}
const liveCount = kind => [...children].filter(record => record.kind === kind).length;
function countChildren() {
  peakSdkChildren = Math.max(peakSdkChildren, liveCount('sdk'));
  peakWebChildren = Math.max(peakWebChildren, liveCount('web'));
  peakChildren = Math.max(peakChildren, children.size);
}

// Linux RSS is sampled for this parent + its workers. Summed RSS can double-count
// shared pages; the separately sampled cgroup value includes the whole cgroup.
let cgroupMemoryPath = null;
if (process.platform === 'linux') {
  try {
    const relative = readFileSync('/proc/self/cgroup', 'utf8').split('\n')
      .find(line => line.startsWith('0::'))?.slice(3);
    const paths = [relative && join('/sys/fs/cgroup', relative, 'memory.current'), '/sys/fs/cgroup/memory.current'];
    cgroupMemoryPath = paths.find(path => {
      try { return path && Number.isFinite(Number(readFileSync(path, 'utf8'))); } catch { return false; }
    }) ?? null;
  } catch { /* RSS remains available on cgroup v1 hosts. */ }
}
function sampleMemory() {
  if (process.platform !== 'linux') return;
  let total = 0;
  for (const pid of [process.pid, ...[...children].map(record => record.child.pid)]) {
    if (!pid) continue;
    try {
      total += Number(readFileSync(`/proc/${pid}/status`, 'utf8').match(/^VmRSS:\s+(\d+) kB$/m)?.[1] ?? 0) * 1024;
    } catch { /* A worker may have exited between enumeration and sampling. */ }
  }
  sampledProcessRssPeakBytes = Math.max(sampledProcessRssPeakBytes ?? 0, total);
  if (cgroupMemoryPath) {
    try {
      sampledCgroupMemoryPeakBytes = Math.max(sampledCgroupMemoryPeakBytes ?? 0,
        Number(readFileSync(cgroupMemoryPath, 'utf8')));
    } catch { /* The containing cgroup may no longer be readable. */ }
  }
}

const workdir = await mkdtemp(join(tmpdir(), 'memhot-concurrency-smoke-'));
const guardPath = join(workdir, 'no-network.mjs');
// Production transports already use host RPC. Fail the probe if a dependency
// nevertheless attempts TCP/TLS/UDP; DNS preflight uses the existing fixture.
await writeFile(guardPath, `import net from 'node:net';
import tls from 'node:tls';
import dgram from 'node:dgram';
import { syncBuiltinESMExports } from 'node:module';
const denied = () => {
  process.stdout.write(JSON.stringify({ type: 'network-forbidden' }) + '\\n');
  throw new Error('Offline concurrency probe forbids network access.');
};
net.Socket.prototype.connect = denied;
tls.connect = denied;
dgram.Socket.prototype.send = denied;
globalThis.fetch = denied;
syncBuiltinESMExports();
`, { mode: 0o600 });

function launch(kind, name, script) {
  if (failure) throw failure;
  const child = spawn(process.execPath, ['--max-old-space-size=128', '--import', guardPath,
    '--import', './test/fixtures/web-no-network.mjs', script], {
    cwd: root,
    env: kind === 'web' ? { MEMHOT_WEB_WORKDIR: workdir, HOME: workdir } : {},
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const record = { child, kind, name };
  record.closed = new Promise(resolve => child.once('close', (code, signal) => {
    children.delete(record);
    resolve({ code, signal });
  }));
  child.once('error', fail);
  child.stdin.on('error', error => { if (child.exitCode === null && !failure) fail(error); });
  children.add(record); allChildren.push(record); countChildren();
  return record;
}
function exchange(record, start, respond) {
  const task = (async () => {
    const { child, name } = record;
    let terminal, stderr = '';
    const pending = new Set();
    child.stderr.on('data', data => { stderr = (stderr + data).slice(-64_000); });
    const lines = createInterface({ input: child.stdout });
    lines.on('line', line => {
      const work = (async () => {
        const frame = JSON.parse(line);
        firstFrameMs[name] ??= elapsed();
        if (frame.type === 'network-forbidden') {
          deniedNetworkAttempts++;
          throw new Error(`${name} attempted network access`);
        }
        if (['model', 'tool', 'http'].includes(frame.type)) {
          assert.ok(!terminal, `${name}: RPC after terminal frame`);
          const value = await wait(respond(frame));
          if (failure) throw failure;
          child.stdin.write(`${JSON.stringify({ type: 'result', id: frame.id, value })}\n`);
        } else if (['complete', 'failed'].includes(frame.type)) {
          assert.ok(!terminal, `${name}: duplicate terminal frame`);
          terminal = frame;
          if (frame.type === 'failed') throw new Error(`${name}: ${frame.code}`);
        } else assert.equal(frame.type, 'checkpoint', `${name}: unexpected frame`);
      })().catch(fail);
      pending.add(work); work.finally(() => pending.delete(work));
    });
    child.stdin.write(`${JSON.stringify(start)}\n`);
    const status = await record.closed;
    // A crash must release barriers before awaiting any blocked RPC responders.
    if (status.code !== 0 || terminal?.type !== 'complete' || stderr) {
      fail(new Error(`${name}: exit=${status.code} signal=${status.signal} terminal=${terminal?.type}; ${stderr}`));
    }
    await Promise.all(pending);
    lines.close();
    if (failure) throw failure;
    return terminal;
  })();
  task.catch(fail); exchanges.push(task);
  return task;
}
function sse(tool, lane) {
  const delta = tool ? { role: 'assistant', tool_calls: [{ index: 0, id: `${lane}-fixture-tool`, type: 'function',
    function: { name: 'read_source', arguments: '{"id":"fixture-source"}' } }] }
    : { role: 'assistant', content: JSON.stringify({ text: `${lane}: verified SQLite fixture evidence.` }) };
  const chunk = (value, finish_reason) => ({ id: `${lane}-fixture-model`, object: 'chat.completion.chunk', created: 1,
    model: 'test-model', choices: [{ index: 0, delta: value, finish_reason }] });
  return { contentType: 'text/event-stream', body: [chunk(delta, null), chunk({}, tool ? 'tool_calls' : 'stop'), '[DONE]']
    .map(value => `data: ${typeof value === 'string' ? value : JSON.stringify(value)}\n\n`).join('') };
}

let sharedSource;
function readSharedSource() {
  // One shared read gate, coalescing the three consumers of this exact source.
  sharedSource ??= (async () => {
    await wait(allReaders.promise);
    sdkChildrenAtWebStart = liveCount('sdk');
    assert.equal(sdkChildrenAtWebStart, 4);
    webStartedMs = elapsed(); webWorkers++;
    const web = launch('web', 'web', 'src/web-main.mjs');
    const paragraph = 'SQLite reads and writes database files directly. This public fixture demonstrates an embedded database. ';
    const html = '<!doctype html><html><head><title>SQLite fixture</title></head><body><main><article><h1>SQLite fixture</h1>'
      + `<p>${paragraph}</p>`.repeat(2000) + '</article></main></body></html>';
    const source = await exchange(web, { type: 'start', action: 'read', url: 'https://sqlite.org/serverless.html' }, frame => {
      assert.equal(frame.type, 'http');
      assert.equal(frame.url, 'https://sqlite.org/serverless.html');
      assert.equal(frame.method, 'GET');
      webFirstFrameMs ??= elapsed() - webStartedMs;
      sdkChildrenAtWebHttp = liveCount('sdk');
      assert.equal(sdkChildrenAtWebHttp, 4); webCalls++;
      return { status: 200, contentType: 'text/html', body: html, url: frame.url };
    });
    assert.equal(source.sources.length, 1);
    assert.equal(source.sources[0].fulltext, true);
    assert.ok(source.sources[0].text.includes('SQLite'));
    sdkChildrenAtWebComplete = liveCount('sdk');
    assert.equal(sdkChildrenAtWebComplete, 4);
    webCompletedMs = elapsed(); sampleMemory(); webDone.resolve();
    return { materials: source.sources };
  })();
  return sharedSource;
}

const timer = setTimeout(() => fail(new Error('Concurrency probe exceeded 180000ms')), 180_000);
const memoryTimer = setInterval(sampleMemory, 100);
const onSignal = signal => fail(new Error(`Concurrency probe received ${signal}`));
const onInt = () => onSignal('SIGINT'), onTerm = () => onSignal('SIGTERM');
process.once('SIGINT', onInt); process.once('SIGTERM', onTerm);
try {
  const runs = lanes.map(lane => {
    modelCallsByLane[lane] = 0; toolCallsByLane[lane] = 0;
    const translation = lane === 'translation';
    return exchange(launch('sdk', lane, 'src/main.mjs'), {
      type: 'start', runId: `concurrency-fixture-${lane}`, mode: lane === 'chat' ? 'discussion' : lane,
      model: { protocol: 'openai', model: 'test-model' },
      prompt: translation ? 'Translate: SQLite reads database files directly.' : 'Read the fixture and return the required JSON.',
      instructions: translation ? 'Return JSON {"text":"translation"}.' : 'Read the source, then return JSON {"text":"answer"}.',
      tools: translation ? [] : [{ name: 'read_source', description: 'Read the registered fixture source', inputSchema: {
        type: 'object', properties: { id: { type: 'string' } }, required: ['id'], additionalProperties: false,
      } }], maxSteps: translation ? 1 : 2, maxOutputTokens: 2000,
    }, async frame => {
      if (frame.type === 'model') {
        const call = ++modelCallsByLane[lane];
        assert.ok(call <= (translation ? 1 : 2));
        if (call === 1) {
          firstModelFrameMs[lane] = elapsed(); ready.add(lane);
          if (ready.size === 4) {
            assert.equal(liveCount('sdk'), 4);
            timeToAllReadyMs = elapsed(); allReady.resolve();
          }
          await wait(allReady.promise);
        }
        // Translation stays at its sole model RPC throughout plugin extraction.
        if (translation) {
          assert.equal(frame.body.tool_choice, 'none');
          assert.ok(!frame.body.tools?.length);
          await wait(webDone.promise);
        }
        return sse(!translation && call === 1, lane);
      }
      assert.equal(frame.type, 'tool'); assert.equal(frame.name, 'read_source');
      assert.deepEqual(frame.args, { id: 'fixture-source' });
      assert.equal(++toolCallsByLane[lane], 1); assert.ok(!translation);
      readers.add(lane); if (readers.size === 3) allReaders.resolve();
      return readSharedSource();
    }).then(result => {
      assert.equal(result.steps, translation ? 1 : 2);
      assert.equal(JSON.parse(result.text).text, `${lane}: verified SQLite fixture evidence.`);
      completed++; return result;
    });
  });
  // Attach rejection handlers to every run before a single child can finish.
  await Promise.all(runs);
  assert.equal(completed, 4); assert.equal(webWorkers, 1); assert.equal(webCalls, 1);
  assert.equal(peakSdkChildren, 4); assert.equal(peakWebChildren, 1); assert.equal(peakChildren, 5);
  assert.equal(deniedNetworkAttempts, 0);
  assert.deepEqual(modelCallsByLane, { chat: 2, translation: 1, digest: 2, research: 2 });
  assert.deepEqual(toolCallsByLane, { chat: 1, translation: 0, digest: 1, research: 1 });
  sampleMemory();
  console.log(JSON.stringify({ passed: true, completed, modelCalls: 7, toolCalls: 3,
    modelCallsByLane, toolCallsByLane, webWorkers, webCalls, paidCalls: 0, networkCalls: 0, deniedNetworkAttempts,
    concurrencyPeak: peakChildren, peakSdkChildren, peakWebChildren,
    sdkChildrenAtWebStart, sdkChildrenAtWebHttp, sdkChildrenAtWebComplete,
    timeToAllReadyMs, firstFrameMs, firstModelFrameMs, webStartedMs, webFirstFrameMs, webCompletedMs,
    elapsedMs: elapsed(), memorySampleIntervalMs: 100, sampledProcessRssPeakBytes,
    sampledCgroupMemoryPeakBytes, cgroupMemoryPath }));
} catch (error) {
  fail(error);
  console.error(JSON.stringify({ passed: false, error: failure.message, completed,
    modelCallsByLane, toolCallsByLane, timeToAllReadyMs, firstFrameMs, elapsedMs: elapsed() }));
  process.exitCode = 1;
} finally {
  clearTimeout(timer); clearInterval(memoryTimer);
  killChildren();
  await Promise.all(allChildren.map(record => record.closed));
  await Promise.allSettled(exchanges);
  process.off('SIGINT', onInt); process.off('SIGTERM', onTerm);
  await rm(workdir, { recursive: true, force: true });
}
