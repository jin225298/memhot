import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { HarnessError } from './errors.mjs';
import { isObject, LIMITS } from './validation.mjs';
import { prepareTransformCache } from './web-transform-cache.mjs';
import { decodePdf, extractPdf } from './web-pdf.mjs';

export const WEB_LIMITS = Object.freeze({ ...LIMITS, inputFrame: 12 * 1024 * 1024,
  outputFrame: 256 * 1024, totalOutput: 16 * 1024 * 1024 - 1024, rpcTimeoutMs: 60_000 });
const invalid = () => { throw new HarnessError('INVALID_START', 'The web tool input is invalid.'); };
const string = (value, max) => typeof value === 'string' && value.trim().length > 0 && value.length <= max;
const blockedHost = host => /^(?:localhost|0\.0\.0\.0|127\.|10\.|169\.254\.|192\.168\.|\[?::1\]?$)/i.test(host) ||
  /^(?:172\.(?:1[6-9]|2\d|3[01])\.|\[?f[cd][\da-f]{2}:|\[?fe[89ab][\da-f]:)/i.test(host) || /\.(?:localhost|local)$/i.test(host);

function httpUrl(value) {
  if (!string(value, 8192)) invalid();
  let url;
  try { url = new URL(value); } catch { invalid(); }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || blockedHost(url.hostname)) invalid();
  url.hash = '';
  return url;
}

function readUrl(value) {
  const url = httpUrl(value);
  if (/(?:^|\.)(?:github\.com|youtube\.com|youtu\.be)$/i.test(url.hostname) ||
    /\.(?:mp[34]|mkv|mov|webm|wav|ogg|avi|jpe?g|png|gif|webp|zip|gz|tar|exe)$/i.test(url.pathname)) invalid();
  return url.toString();
}

export function validateWebStart(value) {
  if (!isObject(value) || value.type !== 'start' || !['search', 'read'].includes(value.action)) invalid();
  const allowed = value.action === 'search' ? ['type', 'action', 'query', 'provider', 'endpoint'] : ['type', 'action', 'url'];
  if (Object.keys(value).some(key => !allowed.includes(key))) invalid();
  if (value.action === 'read') return { ...value, url: readUrl(value.url) };
  if (!string(value.query, 2000) || !['degoog', 'searxng'].includes(value.provider)) invalid();
  let endpoint;
  if (value.endpoint !== undefined) {
    try {
      const url = new URL(value.endpoint);
      if (value.provider !== 'searxng' || url.protocol !== 'http:' || url.hostname !== '127.0.0.1' ||
        !url.port || url.username || url.password || url.search || url.hash || !['/', '/search'].includes(url.pathname)) invalid();
      endpoint = url.origin;
    } catch { invalid(); }
  }
  if (value.provider === 'searxng' && !endpoint) invalid();
  return { ...value, query: value.query.trim(), ...(endpoint ? { endpoint } : {}) };
}

function challenge(contentType, body) {
  return /(?:html|xhtml)/i.test(contentType) &&
    (/<title>\s*(?:Just a moment|Access denied|Attention required|Verify (?:you|your)|Security (?:check|verification))/i.test(body) ||
      /(?:__cf_chl|cf-chl-|id=["'](?:challenge-form|captcha))/i.test(body));
}

/** Calls the installed plugin's standalone core; Rust remains the sole HTTP transport. */
export async function runWeb(start, channel, signal) {
  const parentDir = process.env.MEMHOT_WEB_WORKDIR || tmpdir();
  const sandboxDir = await mkdtemp(join(parentDir, 'memhot-web-'));
  // No provider credentials, proxies, browser profiles, or global Pi configuration may be inherited.
  for (const key of Object.keys(process.env)) delete process.env[key];
  process.env.PI_CODING_AGENT_DIR = sandboxDir;
  const config = {
    searchProvider: start.provider ?? 'degoog', workflow: 'standard', maxInlineContentChars: 48_000,
    webSearch: { allowedProviders: ['degoog', 'searxng'] },
    tools: { webSearch: { enabled: true }, fetchContent: { enabled: true },
      sourceCheck: { enabled: false }, getSearchContent: { enabled: false } },
    fetch: { defaultMode: 'readable', allowedModes: ['readable'] },
    fetchRouting: { providers: ['http'], allowRemoteHostedProviders: false },
    githubClone: { enabled: false }, githubPrIssue: { enabled: false }, youtube: { enabled: false },
    video: { enabled: false }, pdf: { enabled: false }, image: { enabled: false },
    ...(start.endpoint ? { searxngBaseUrl: start.endpoint, ssrf: { allowRanges: ['127.0.0.1/32'] } } : {}),
  };
  let transportFailure, requests = 0, fetchedUrl, readResponse;
  globalThis.fetch = async (input, init = {}) => {
    if (transportFailure) throw transportFailure;
    try {
      const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url);
      const method = (init.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();
      const allowedUrl = start.action === 'read' ? url.toString() === start.url && method === 'GET'
        : start.provider === 'degoog' ? url.toString() === 'https://degoog.org/api/search' && method === 'POST'
        : url.origin === start.endpoint && url.pathname === '/search' && method === 'GET';
      if (!allowedUrl || ++requests > (start.action === 'read' ? 2 : 1)) {
        throw new HarnessError('NETWORK_FORBIDDEN', 'The plugin requested an unavailable HTTP operation.');
      }
      if (start.action === 'read' && readResponse) return readResponse.clone();
      const headers = {};
      for (const [name, value] of new Headers(init.headers ?? (input instanceof Request ? input.headers : undefined))) {
        if (['accept', 'content-type', 'user-agent'].includes(name)) headers[name] = value;
      }
      if (start.action === 'read') headers['user-agent'] = 'Memhot/0.3';
      const body = init.body;
      if (body !== undefined && (typeof body !== 'string' || Buffer.byteLength(body) > 16 * 1024)) {
        throw new HarnessError('HTTP_REQUEST', 'The plugin HTTP request is invalid.');
      }
      const result = await channel.request('http', { url: url.toString(), method, headers, ...(body === undefined ? {} : { body }) }, signal);
      if (!isObject(result) || !Number.isInteger(result.status) || typeof result.body !== 'string' ||
        typeof result.contentType !== 'string' || typeof result.url !== 'string' || result.contentType.length > 200) {
        throw new HarnessError('HTTP_RESPONSE', 'The host HTTP response is invalid.');
      }
      const pdf = start.action === 'read' && /^application\/pdf(?:;|$)/i.test(result.contentType);
      if (!pdf && Buffer.byteLength(result.body) > 2 * 1024 * 1024) throw new HarnessError('HTTP_LIMIT', 'The HTTP response exceeded its limit.');
      if (result.status < 200 || result.status >= 300 || challenge(result.contentType, result.body)) {
        throw new HarnessError('HTTP_DENIED', 'The source did not permit this request.');
      }
      if (start.action === 'read' && !pdf && !/^(?:text\/(?:html|plain|(?:x-)?markdown)|application\/(?:xhtml\+xml|json))(?:;|$)/i.test(result.contentType)) {
        throw new HarnessError('HTTP_CONTENT', 'The source did not return a supported document.');
      }
      const response = new Response(pdf ? decodePdf(result.body, result.bodyEncoding) : result.body, { status: result.status, headers: { 'content-type': result.contentType } });
      if (start.action === 'search' && result.url !== url.toString()) {
        throw new HarnessError('HTTP_RESPONSE', 'The search endpoint returned an unavailable redirect.');
      }
      fetchedUrl = start.action === 'read' ? httpUrl(result.url).toString() : result.url;
      Object.defineProperty(response, 'url', { value: fetchedUrl });
      if (start.action === 'read') readResponse = response.clone();
      return response;
    } catch (error) {
      transportFailure = error instanceof HarnessError ? error : new HarnessError('HTTP_FAILED', 'The HTTP request failed.');
      throw transportFailure;
    }
  };
  try {
    if (start.action === 'read') {
      const response = await globalThis.fetch(start.url);
      if (/^application\/pdf(?:;|$)/i.test(response.headers.get('content-type'))) {
        const extracted = await extractPdf(new Uint8Array(await response.arrayBuffer()), signal);
        const fallback = new URL(fetchedUrl).pathname.split('/').pop() || 'Public PDF';
        return { type: 'complete', sources: [{ title: extracted.title || fallback, url: fetchedUrl,
          text: extracted.text, fulltext: true, extraction: { format: 'pdf', truncated: extracted.truncated,
          pagesRead: extracted.pagesRead, totalPages: extracted.totalPages } }] };
      }
    }
    await writeFile(join(sandboxDir, 'web-search.json'), JSON.stringify(config), { mode: 0o600, flag: 'wx' });
    // Only verified static transforms are copied into this child. Config, queries,
    // responses and plugin runtime state remain in its disposable sandbox.
    const { createJiti } = await import('jiti');
    const jiti = createJiti(import.meta.url, { fsCache: await prepareTransformCache(sandboxDir), moduleCache: true });
    const { createStandaloneWebToolCore } = await jiti.import(import.meta.resolve('pi-web-access/web-tool-core.ts'));
    const core = createStandaloneWebToolCore();
    const result = start.action === 'search'
      ? await core.webSearch({ queries: [start.query], provider: start.provider, numResults: 20, includeContent: false }, signal)
      : await core.fetchContent({ url: start.url, mode: 'readable' }, signal);
    if (transportFailure) throw transportFailure;
    if (channel.error) throw channel.error;
    const data = result.structuredContent;
    if (result.isError || !isObject(data)) throw new HarnessError('WEB_PLUGIN_FAILED', 'The web plugin could not retrieve this source.');
    const sources = [];
    if (start.action === 'search') {
      if (!Array.isArray(data.queries) || data.queries.some(query => query.error)) {
        throw new HarnessError('WEB_PLUGIN_FAILED', 'The web plugin search failed.');
      }
      for (const query of data.queries) for (const item of query.results ?? []) {
        try {
          sources.push({ title: String(item.title).slice(0, 1024), url: httpUrl(item.url).toString(),
            text: String(item.snippet).slice(0, 2000), fulltext: false });
        } catch { /* Unusable result URLs cannot become evidence. */ }
      }
    } else {
      if (!Array.isArray(data.urls) || data.urls.length !== 1 || data.urls[0].error || !string(data.urls[0].content, 2 * 1024 * 1024)) {
        throw new HarnessError('WEB_PLUGIN_FAILED', 'The web plugin did not extract usable document text.');
      }
      const item = data.urls[0];
      sources.push({ title: String(item.title).slice(0, 1024), url: fetchedUrl ?? httpUrl(item.url).toString(),
        text: item.content.slice(0, 48_000), fulltext: true });
    }
    return { type: 'complete', sources };
  } finally { await rm(sandboxDir, { recursive: true, force: true }); }
}
