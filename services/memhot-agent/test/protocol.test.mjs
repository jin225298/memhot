import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough, Writable } from 'node:stream';
import { JsonlChannel } from '../src/protocol.mjs';
import { LIMITS } from '../src/validation.mjs';

function fixture(overrides = {}) {
  const input = new PassThrough();
  const lines = [];
  const output = new Writable({ write(chunk, _, callback) { lines.push(JSON.parse(chunk.toString())); callback(); } });
  const channel = new JsonlChannel(input, output, { ...LIMITS, ...overrides });
  input.write('{"type":"start"}\n');
  return { input, lines, output, channel };
}

// Catches resolving a concurrent RPC with the wrong tool result.
test('RPC ids correlate out-of-order host responses', async () => {
  const { input, lines, channel } = fixture();
  await channel.start;
  const first = channel.request('tool', { name: 'search', args: { query: 'first' } });
  const second = channel.request('tool', { name: 'search', args: { query: 'second' } });
  input.write(`${JSON.stringify({ type: 'result', id: lines[1].id, value: 'second result' })}\n`);
  input.write(`${JSON.stringify({ type: 'result', id: lines[0].id, value: 'first result' })}\n`);
  assert.equal(await first, 'first result');
  assert.equal(await second, 'second result');
  channel.close();
});

// Catches indefinitely waiting on dead host/model requests and leaking RPC timers.
test('pending requests time out and future requests fail immediately', async () => {
  const { channel } = fixture({ rpcTimeoutMs: 10 });
  await channel.start;
  await assert.rejects(channel.request('model', { body: {} }), { code: 'RPC_TIMEOUT' });
  await assert.rejects(channel.request('model', { body: {} }), { code: 'RPC_TIMEOUT' });
  assert.equal(channel.pending.size, 0);
  channel.close();
});

// Catches using the provider error text as a model-readable result or user-facing stack trace.
test('host errors are sanitized and never become success results', async () => {
  const { input, lines, channel } = fixture();
  await channel.start;
  const request = channel.request('model', { body: {} });
  input.write(`${JSON.stringify({ type: 'error', id: lines[0].id, code: 'HTTP401', message: 'sk-secret full provider response' })}\n`);
  await assert.rejects(request, error => error.code === 'MODEL_FAILED' && !error.message.includes('sk-secret'));
  channel.close();
});

// Catches accepting orphan/duplicate responses or letting pending calls hang on protocol failure.
test('unexpected ids and duplicate result frames fail the connection', async () => {
  const { input, channel } = fixture();
  await channel.start;
  const pending = channel.request('tool', { name: 'read', args: {} });
  input.write('{"type":"result","id":"orphan","value":{}}\n');
  await assert.rejects(pending, { code: 'RPC_PROTOCOL' });
  assert.equal(channel.pending.size, 0);
  channel.close();
});

// Catches output flooding and inability to report an output-limit terminal failure.
test('bounded output still permits a terminal failure frame', async () => {
  const { lines, channel } = fixture({ outputFrame: 256, totalOutput: 100 });
  await channel.start;
  await assert.rejects(channel.emit({ type: 'checkpoint', messages: ['x'.repeat(110)] }), { code: 'OUTPUT_LIMIT' });
  await channel.emit({ type: 'failed', code: 'OUTPUT_LIMIT', message: 'Output limit.' }, true);
  assert.equal(lines.length, 1);
  assert.equal(lines[0].type, 'failed');
  channel.close();
});

// Catches UTF-8 corruption, unbounded partial frames, or dropping lines split across pipe chunks.
test('JSONL parser handles split UTF-8 and rejects an oversized unterminated frame', async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const channel = new JsonlChannel(input, output, { ...LIMITS, inputFrame: 64 });
  const frame = Buffer.from('{"type":"start","text":"中文"}\n');
  input.write(frame.subarray(0, 25));
  input.write(frame.subarray(25));
  assert.equal((await channel.start).text, '中文');
  const pending = channel.request('model', { body: {} });
  input.write('x'.repeat(65));
  await assert.rejects(pending, { code: 'INPUT_LIMIT' });
  channel.close();
});

// Catches a successful terminal frame bypassing the cumulative stdout budget.
test('complete frames count toward the total output limit', async () => {
  const { lines, channel } = fixture({ outputFrame: 256, totalOutput: 100 });
  await channel.start;
  await channel.emit({ type: 'checkpoint', text: 'x'.repeat(25) });
  await assert.rejects(channel.emit({ type: 'complete', text: 'y'.repeat(70) }, true), { code: 'OUTPUT_LIMIT' });
  assert.equal(lines.length, 1);
  channel.close();
});

// Catches a host that stops reading stdout leaving the agent blocked forever.
test('blocked stdout writes have a bounded wait', async () => {
  const input = new PassThrough();
  const output = new Writable({ write() {} });
  const channel = new JsonlChannel(input, output, { ...LIMITS, rpcTimeoutMs: 10 });
  input.write('{"type":"start"}\n');
  await channel.start;
  await assert.rejects(channel.emit({ type: 'checkpoint' }), { code: 'RPC_TIMEOUT' });
  channel.close();
});

test('isolated cleanup cancellation preserves the channel and ignores its late response once', async () => {
  const { input, lines, channel } = fixture();
  await channel.start;
  const controller = new AbortController();
  const cleanup = channel.request('http', { method: 'DELETE' }, controller.signal, { isolatedCancellation: true });
  controller.abort();
  await assert.rejects(cleanup, { code: 'RUN_TIMEOUT' });
  assert.equal(channel.error, undefined);
  assert.equal(channel.pending.size, 0);
  input.write(`${JSON.stringify({ type: 'result', id: lines[0].id, value: {} })}\n`);
  assert.equal(channel.error, undefined);
  await channel.emit({ type: 'complete', sources: [] }, true);
  input.write(`${JSON.stringify({ type: 'result', id: lines[0].id, value: {} })}\n`);
  assert.equal(channel.error.code, 'RPC_PROTOCOL');
  channel.close();
});
