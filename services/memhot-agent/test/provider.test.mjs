import test from 'node:test';
import assert from 'node:assert/strict';
import { createProvider } from '../src/provider.mjs';

const start = { model: { protocol: 'openai', model: 'test-model' }, maxOutputTokens: 1000 };

// Catches accidentally falling back to global network access or forwarding a child-selected URL.
test('Pi transport rejects arbitrary networking and forwards only raw model bodies', async () => {
  const original = globalThis.fetch;
  const requests = [];
  try {
    await createProvider(start, { request: async (kind, payload) => {
      requests.push({ kind, payload });
      return { body: 'data: [DONE]\n\n', contentType: 'text/event-stream' };
    } });
    await assert.rejects(fetch('https://example.com/private', { method: 'POST', body: '{}' }), { code: 'NETWORK_FORBIDDEN' });
    await assert.rejects(fetch('https://memhot.invalid/v1/models', { method: 'GET' }), { code: 'NETWORK_FORBIDDEN' });
    const response = await fetch('https://memhot.invalid/v1/chat/completions', { method: 'POST', body: '{"model":"test-model","stream":true}' });
    assert.equal(await response.text(), 'data: [DONE]\n\n');
    assert.deepEqual(requests, [{ kind: 'model', payload: { body: { model: 'test-model', stream: true } } }]);
  } finally { globalThis.fetch = original; }
});

// Catches unbounded/incorrect RPC response bodies being passed to provider parsers.
test('model transport rejects non-SSE and oversized host responses', async () => {
  const original = globalThis.fetch;
  try {
    for (const value of [{ body: '{}', contentType: 'application/json' }, { body: 'x'.repeat(4 * 1024 * 1024 + 1), contentType: 'text/event-stream' }]) {
      await createProvider(start, { request: async () => value });
      await assert.rejects(fetch('https://memhot.invalid/v1/chat/completions', { method: 'POST', body: '{}' }), { code: 'MODEL_RESPONSE' });
    }
  } finally { globalThis.fetch = original; }
});

// Catches rejecting valid high-overhead SSE even though provider output tokens remain bounded.
test('model transport accepts the full four MiB raw SSE boundary', async () => {
  const original = globalThis.fetch;
  try {
    const body = `: ${'x'.repeat(4 * 1024 * 1024 - 4)}\n\n`;
    await createProvider(start, { request: async () => ({ body, contentType: 'text/event-stream' }) });
    const response = await fetch('https://memhot.invalid/v1/chat/completions', { method: 'POST', body: '{}' });
    assert.equal((await response.text()).length, 4 * 1024 * 1024);
  } finally { globalThis.fetch = original; }
});
