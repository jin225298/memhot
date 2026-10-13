import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { PassThrough, Writable } from 'node:stream';
import { JsonlChannel } from '../src/protocol.mjs';
import { AIHOT_ENDPOINT, parseAiHotResult, AIHOT_PROFILE, runAiHot, validateAiHotStart } from '../src/aihot-worker.mjs';
import { rpcFetch as transportFetch } from '../src/mcp-readonly.mjs';
const rpcFetch = (start, channel, signal) => transportFetch(start, channel, signal, AIHOT_PROFILE);
const fixture = JSON.parse(await readFile(new URL('./fixtures/aihot-daily.json', import.meta.url)));
const start = { type:'start', tool:'aihot_get_daily', args:{} };

test('official Pi MCP performs initialize/list/call/close through the injected host RPC', async () => {
  const methods = [];
  const channel = { async request(kind, value) {
    assert.equal(kind, 'http'); assert.equal(value.url, AIHOT_ENDPOINT);
    let status = 200, body;
    if (value.method === 'DELETE') { methods.push('DELETE'); status = 204; body = ''; }
    else {
      const rpc = JSON.parse(value.body); methods.push(rpc.method);
      let result;
      if (rpc.method === 'initialize') result = { protocolVersion:'2025-11-25', capabilities:{tools:{}}, serverInfo:{name:'fixture',version:'1'} };
      else if (rpc.method === 'notifications/initialized') { status = 202; body = ''; }
      else if (rpc.method === 'tools/list') result = {tools:[{name:start.tool,inputSchema:{type:'object',properties:{}},annotations:{readOnlyHint:true}}]};
      else if (rpc.method === 'tools/call') { assert.deepEqual(rpc.params,{name:start.tool,arguments:{}}); result = fixture; }
      else throw new Error('unexpected method');
      if (result) body = JSON.stringify({jsonrpc:'2.0',id:rpc.id,result});
    }
    return { status,body,contentType:'application/json',url:AIHOT_ENDPOINT,headers:{'mcp-session-id':'fixture-session'} };
  }};
  const result = await runAiHot(start, channel, new AbortController().signal);
  assert.deepEqual(methods, ['initialize','notifications/initialized','tools/list','tools/call','DELETE']);
  assert.equal(result.sources[0].url, 'https://example.org/original');
  assert.equal(result.sources[0].attribution.url, 'https://aihot.news/items/fixture');
  assert.equal(result.sources[0].fulltext, false);
  assert.equal(result.overview.headline, 'Synthetic AI daily headline');
  assert.equal(result.overview.attribution.url, 'https://aihot.news/daily/2026-10-12');
  assert.equal(result.overview.period.date, '2026-10-12');
  assert.match(result.overview.warning, /不是已核对原文证据/);
});

test('input, untrusted attribution, malformed schema and remote errors fail closed', () => {
  for (const value of [{...start,tool:'run_shell'},{...start,endpoint:'https://evil.invalid'},{...start,args:[]}])
    assert.throws(() => validateAiHotStart(value));
  for (const result of [{isError:true}, {...fixture,structuredContent:{schemaVersion:2}}, {content:[{type:'text',text:'unstructured'}]}])
    assert.throws(() => parseAiHotResult(result,start.tool));
  const altered = structuredClone(fixture);
  altered.structuredContent.report.sections[0].items[0].attribution.url='https://evil.invalid/';
  assert.equal(parseAiHotResult(altered,start.tool).sources.length,0);
});

test('transport denies alternate endpoints, tool arguments and sensitive headers before RPC', async () => {
  let count = 0;
  const channel = {request:async()=>{count++;throw Error('unexpected');}};
  for (const [url, init] of [[AIHOT_ENDPOINT+'?private=secret',{method:'POST',body:'{}'}],
    [AIHOT_ENDPOINT,{method:'GET'}],[AIHOT_ENDPOINT,{method:'POST',headers:{authorization:'secret'},body:'{}'}],
    [AIHOT_ENDPOINT,{method:'POST',body:JSON.stringify({method:'tools/call',params:{name:'aihot_search',arguments:{q:'private'}}})}]]) {
    const rpc = rpcFetch(start,channel);
    await assert.rejects(rpc.fetch(url,init));
  }
  assert.equal(count,0);
});

test('429 and malformed host responses stop immediately without retries', async () => {
  for (const value of [{status:429,body:'rate limited',headers:{},url:AIHOT_ENDPOINT,contentType:'application/json'},
    {status:200,body:'{}',headers:{},url:'https://evil.invalid/',contentType:'application/json'},
    {status:200,body:'x'.repeat(512*1024+1),headers:{},url:AIHOT_ENDPOINT,contentType:'application/json'}]) {
    let count=0;
    const rpc = rpcFetch(start,{request:async()=>{count++;return value;}});
    await assert.rejects(rpc.fetch(AIHOT_ENDPOINT,{method:'POST',body:'{}'}));
    await assert.rejects(rpc.fetch(AIHOT_ENDPOINT,{method:'POST',body:'{}'}));
    assert.equal(count,1);
  }
});

test('story IDs are derived only from hot-topic links and summaries remain discoveries', () => {
  const result={structuredContent:{schemaVersion:1,topics:[{title:'Synthetic event',links:{story:'https://aihot.news/stories/known-event'}}]},content:[]};
  assert.deepEqual(parseAiHotResult(result,'aihot_get_hot_topics').stories,[{id:'known-event',url:'https://aihot.news/stories/known-event'}]);
  assert.deepEqual(parseAiHotResult(result,'aihot_get_daily').stories,[]);
});

test('weekly/monthly headline, overview and section context stay bounded and separate from evidence', () => {
  const weekly = structuredClone(fixture);
  delete weekly.structuredContent.report.lead;
  const report = weekly.structuredContent.report;
  report.headline = 'Synthetic weekly headline'; report.overview = 'x'.repeat(6000);
  report.periodStart = '2026-10-05'; report.periodEnd = '2026-10-11';
  report.sections[0].summary = 'Synthetic section context';
  const result = parseAiHotResult(weekly, 'aihot_get_weekly');
  assert.equal(result.overview.headline, report.headline);
  assert.equal(result.overview.summary.length, 1000);
  assert.equal(result.overview.period.periodEnd, '2026-10-11');
  assert.equal(result.overview.sections[0].summary, 'Synthetic section context');
  assert.equal(result.sources[0].fulltext, false);
  assert.equal(result.overview.evidenceId, undefined);
});

test('SDK abort signals prevent already-cancelled HTTP and independently bound session cleanup', async () => {
  let calls = 0;
  const rpc = rpcFetch(start, { request: async () => { calls++; } }, new AbortController().signal);
  await assert.rejects(rpc.fetch(AIHOT_ENDPOINT, { method: 'DELETE',
    headers: { 'mcp-session-id': 'fixture' }, signal: AbortSignal.abort() }));
  assert.equal(calls, 0);
  const outer = new AbortController();
  const live = rpcFetch(start, { request: async (_, __, signal, options) => {
    assert.equal(options.isolatedCancellation, true);
    return new Promise((_, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
  } }, outer.signal);
  const timer = setTimeout(() => {}, 100);
  await assert.rejects(live.fetch(AIHOT_ENDPOINT, { method: 'DELETE',
    headers: { 'mcp-session-id': 'fixture' }, signal: AbortSignal.timeout(20) }));
  clearTimeout(timer);
  assert.equal(outer.signal.aborted, false);
  assert.doesNotThrow(() => live.check());
});

test('real Pi SDK hanging DELETE cleanup preserves a completed discovery within its one-second bound', { timeout: 2500 }, async () => {
  const input = new PassThrough();
  let cleanupCalls = 0;
  const output = new Writable({ write(chunk, _, callback) {
    const frame = JSON.parse(chunk.toString());
    if (frame.method === 'DELETE') { cleanupCalls++; callback(); return; }
    const rpc = JSON.parse(frame.body);
    let status = 200, result;
    if (rpc.method === 'initialize') result = { protocolVersion: '2025-11-25', capabilities: { tools: {} }, serverInfo: { name: 'fixture', version: '1' } };
    else if (rpc.method === 'notifications/initialized') status = 202;
    else if (rpc.method === 'tools/list') result = { tools: [{ name: start.tool, inputSchema: { type: 'object' }, annotations: { readOnlyHint: true } }] };
    else if (rpc.method === 'tools/call') result = fixture;
    else throw Error('Unexpected fixture RPC');
    queueMicrotask(() => input.write(`${JSON.stringify({ type: 'result', id: frame.id, value: {
      status, body: result ? JSON.stringify({ jsonrpc: '2.0', id: rpc.id, result }) : '',
      contentType: 'application/json', url: AIHOT_ENDPOINT, headers: { 'mcp-session-id': 'fixture' },
    } })}\n`));
    callback();
  } });
  const channel = new JsonlChannel(input, output);
  input.write(`${JSON.stringify(start)}\n`);
  await channel.start;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 2100);
  const began = performance.now();
  try {
    const result = await runAiHot(start, channel, controller.signal);
    assert.equal(result.sources.length, 1);
    assert.equal(cleanupCalls, 1);
    assert.ok(performance.now() - began < 1800);
    assert.equal(channel.error, undefined);
    assert.equal(channel.pending.size, 0);
  } finally { clearTimeout(timer); channel.close(); }
});
