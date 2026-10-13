import { McpClient, StreamableHttpTransport } from '@earendil-works/pi-mcp';
import { HarnessError } from './errors.mjs';
import { isObject } from './validation.mjs';
const failure = code => new HarnessError(code, 'The paper or news service is unavailable.');

export function rpcFetch(start, channel, signal, profile) {
  let requests = 0, denied;
  const fetch = async (input, init = {}) => {
    if (denied) throw denied;
    const combined = AbortSignal.any([signal, init.signal].filter(Boolean));
    const cleanup = (init.method ?? 'GET').toUpperCase() === 'DELETE';
    try {
      const url = String(input);
      const method = (init.method ?? 'GET').toUpperCase();
      if (url !== profile.endpoint || !['POST', 'DELETE'].includes(method) || ++requests > 7) throw failure('NETWORK_FORBIDDEN');
      const headers = {};
      for (const [name, value] of new Headers(init.headers)) {
        if (!['accept', 'content-type', 'mcp-protocol-version', 'mcp-session-id'].includes(name) || value.length > 512) throw failure('NETWORK_FORBIDDEN');
        headers[name] = value;
      }
      if (method === 'POST') {
        if (typeof init.body !== 'string' || Buffer.byteLength(init.body) > 8192) throw failure('HTTP_REQUEST');
        const body = JSON.parse(init.body);
        if (body.method === 'tools/call' && (body.params?.name !== start.tool ||
          JSON.stringify(body.params?.arguments) !== JSON.stringify(start.args))) throw failure('NETWORK_FORBIDDEN');
      }
      combined.throwIfAborted();
      const result = await channel.request('http', { url, method, headers,
        ...(method === 'POST' ? { body: init.body } : {}) }, combined, { isolatedCancellation: cleanup });
      if (!isObject(result) || !Number.isInteger(result.status) || result.status < 200 || result.status > 299 ||
        result.url !== profile.endpoint || typeof result.body !== 'string' || Buffer.byteLength(result.body) > profile.bodyLimit ||
        typeof result.contentType !== 'string' || !isObject(result.headers)) throw failure('HTTP_RESPONSE');
      if (![202, 204].includes(result.status) && !/^(application\/json|text\/event-stream)(?:;|$)/i.test(result.contentType)) throw failure('HTTP_RESPONSE');
      const responseHeaders = { 'content-type': result.contentType };
      if (typeof result.headers['mcp-session-id'] === 'string' && result.headers['mcp-session-id'].length <= 512)
        responseHeaders['mcp-session-id'] = result.headers['mcp-session-id'];
      return new Response([202, 204].includes(result.status) ? null : result.body, { status: result.status, headers: responseHeaders });
    } catch (error) {
      if (cleanup && combined.aborted) throw error;
      denied = error instanceof HarnessError ? error : failure('HTTP_FAILED');
      throw denied;
    }
  };
  return { fetch, check: () => { if (denied) throw denied; } };
}

export async function runReadonly(start, channel, signal, profile) {
  const rpc = rpcFetch(start, channel, signal, profile);
  const client = new McpClient({ name: profile.client, version: '0.3.3', requestTimeoutMs: profile.timeout });
  const transport = new StreamableHttpTransport({ url: profile.endpoint, fetch: rpc.fetch,
    openGetStream: false, maxMessageBytes: profile.bodyLimit, reconnect: { maxRetries: 0 } });
  try {
    await client.connect(transport);
    const tools = await client.listTools({ signal, timeoutMs: 10_000 });
    const tool = tools.find(tool => tool.name === start.tool);
    if (!tool || tool.annotations?.readOnlyHint !== true || tool.annotations?.destructiveHint === true)
      throw failure('MCP_TOOL');
    const result = await client.callTool(start.tool, start.args, { signal, timeoutMs: profile.timeout });
    rpc.check();
    return profile.parse(result, start);
  } finally { await client.close(); }
}
