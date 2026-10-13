import { getCurrentTools, InMemoryCredentialStore, InMemoryModelsStore } from '@earendil-works/pi-ai';
import { streamSimple as openai } from '@earendil-works/pi-ai/api/openai-completions';
import { streamSimple as anthropic } from '@earendil-works/pi-ai/api/anthropic-messages';
import { streamSimple as gemini } from '@earendil-works/pi-ai/api/google-generative-ai';
import { ModelRuntime } from '@earendil-works/pi-coding-agent';
import { HarnessError } from './errors.mjs';
import { isObject, LIMITS } from './validation.mjs';

const providers = {
  openai: { api: 'openai-completions', stream: openai },
  anthropic: { api: 'anthropic-messages', stream: anthropic },
  gemini: { api: 'google-generative-ai', stream: gemini },
};
const jsonModes = new Set(['digest', 'research', 'translation', 'review']);

/** Pi owns request/response conversion. Rust owns endpoints, authentication, and raw SSE I/O. */
export async function createProvider(start, channel) {
  const provider = providers[start.model.protocol];
  const fetch = async (input, init) => {
    const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url);
    if (url.origin !== 'https://memhot.invalid' || init?.method !== 'POST') {
      throw new HarnessError('NETWORK_FORBIDDEN', 'Only host model transport is permitted.');
    }
    let body;
    try { body = JSON.parse(init.body); } catch { throw new HarnessError('MODEL_REQUEST', 'The model request is invalid.'); }
    if (!isObject(body)) throw new HarnessError('MODEL_REQUEST', 'The model request is invalid.');
    const value = await channel.request('model', { body }, init.signal);
    if (!isObject(value) || typeof value.body !== 'string' || value.contentType !== 'text/event-stream' ||
      Buffer.byteLength(value.body) > LIMITS.modelSse) throw new HarnessError('MODEL_RESPONSE', 'The model response is invalid.');
    return new Response(value.body, { status: 200, headers: { 'content-type': value.contentType } });
  };
  // Google SDK uses global fetch; all network access is replaced by this closed RPC transport.
  // Other Pi adapters also receive the same function explicitly. The child has no real API keys.
  globalThis.fetch = fetch;
  const runtime = await ModelRuntime.create({
    credentials: new InMemoryCredentialStore(), modelsStore: new InMemoryModelsStore(),
    modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  runtime.registerProvider('memhot', {
    api: provider.api, apiKey: 'rpc-only', baseUrl: 'https://memhot.invalid/v1',
    models: [{ id: start.model.model, name: start.model.model, api: provider.api,
      input: ['text'], reasoning: true, contextWindow: 131_072, maxTokens: start.maxOutputTokens,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      ...(start.model.protocol === 'openai' ? { compat: { supportsDeveloperRole: false, supportsStore: false,
        ...(/^deepseek(?:[-/]|$)/i.test(start.model.model) ? { maxTokensField: 'max_tokens', thinkingFormat: 'deepseek',
          requiresReasoningContentOnAssistantMessages: true, supportsReasoningEffort: false } : {}),
      } } : {}),
    }],
    streamSimple: (model, context, options) => {
      // Resolve native tool deltas, including removals on the reserved synthesis turn.
      const wireOptions = start.model.protocol === 'openai' ? {
        ...(getCurrentTools(context.messages).length === 0 ? { toolChoice: 'none' } : {}),
        ...(jsonModes.has(start.mode) ? { samplingParams: { ...options?.samplingParams,
          response_format: { type: 'json_object' } } } : {}),
      } : {};
      return provider.stream(model, context, {
        ...options, ...wireOptions, apiKey: 'rpc-only', env: {}, fetch, transport: 'sse',
        maxRetries: 0, maxTokens: start.maxOutputTokens, timeoutMs: LIMITS.rpcTimeoutMs,
        cacheRetention: 'none',
      });
    },
  });
  return { runtime, model: runtime.getModel('memhot', start.model.model) };
}
