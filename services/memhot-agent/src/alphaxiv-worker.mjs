import { runReadonly } from './mcp-readonly.mjs';
import { HarnessError } from './errors.mjs';
import { isObject, LIMITS } from './validation.mjs';
export const ALPHAXIV_ENDPOINT = 'https://api.alphaxiv.org/mcp/v1';
export const ALPHAXIV_LIMITS = Object.freeze({ ...LIMITS, inputFrame: 3 * 1024 * 1024,
  outputFrame: 512 * 1024, totalOutput: 6 * 1024 * 1024, rpcTimeoutMs: 60_000 });
const tools = ['discover_papers', 'get_paper_content', 'list_library'];
const failure = () => new HarnessError('MCP_RESPONSE', 'Paper content is unavailable.');
export function paperUrl(value) {
  if (typeof value !== 'string' || value.length > 200) return undefined;
  try {
    const url = new URL(value);
    if (url.protocol !== 'https:' || !['arxiv.org', 'www.alphaxiv.org', 'alphaxiv.org'].includes(url.hostname)
        || url.port || url.username || url.password || url.search || url.hash) return undefined;
    const match = /^\/(?:abs|pdf|html)\/((?:\d{4}\.\d{4,5}|[a-z-]+(?:\.[A-Z]{2})?\/\d{7})(?:v[1-9]\d*)?)(?:\.pdf)?$/.exec(url.pathname);
    return match ? `https://arxiv.org/pdf/${match[1]}` : undefined;
  } catch { return undefined; }
}
export function validateAlphaStart(value) {
  if (!isObject(value) || value.type !== 'start' || !tools.includes(value.tool) || !isObject(value.args)
    || Buffer.byteLength(JSON.stringify(value.args)) > 4096
    || Object.keys(value).some(key => !['type','tool','args'].includes(key))) throw failure();
  if (value.tool === 'get_paper_content' && (!paperUrl(value.args.url) || value.args.fullText !== true)) throw failure();
  if (value.tool === 'list_library' && JSON.stringify(value.args) !== '{"include_papers":true}') throw failure();
  return value;
}
export function parseAlphaResult(result, start) {
  if (!isObject(result) || result.isError || !Array.isArray(result.content)) throw failure();
  const blocks = result.content.filter(item => item?.type === 'text' && typeof item.text === 'string');
  const text = blocks.map(item => item.text).join('\n');
  if (!text || Buffer.byteLength(text) > 2 * 1024 * 1024) throw failure();
  if (start.tool === 'get_paper_content') {
    if (start.args.fullText !== true || !paperUrl(start.args.url) || text.length < 200) throw failure();
    return { type: 'complete', text: text.slice(0, 100_000), truncated: text.length > 100_000,
      url: paperUrl(start.args.url), fulltext: true };
  }
  const sources = [], seen = new Set();
  const add = (url, title, excerpt, date) => {
    const canonical = paperUrl(url);
    if (!canonical || seen.has(canonical) || sources.length >= 20) return;
    seen.add(canonical);
    sources.push({ url: canonical, title: String(title || canonical).slice(0, 240),
      text: String(excerpt || '').slice(0, 1400), publishedAt: typeof date === 'string' ? date.slice(0, 40) : null });
  };
  if (start.tool === 'discover_papers') {
    // The live API returns a ranked Markdown list, not a JSON papers envelope.
    for (const line of text.split('\n').slice(0, 100)) {
      const match = /\*\*(.+?)\*\*\s*\((https:\/\/[^\s)]+)\)/.exec(line);
      if (match) add(match[2], match[1], line, /Published (\d{4}-\d{2}-\d{2})/.exec(line)?.[1]);
    }
    if (!sources.length && /https:\/\//.test(text)) throw failure();
    return { type: 'complete', sources, warning: '搜索摘要未核对原文。相关性优先可能返回旧论文，请检查发表日期。' };
  }
  let data;
  try { data = JSON.parse(text); } catch { throw failure(); }
  if (!Array.isArray(data.folders)) throw failure();
  const folders = data.folders.slice(0, 30).map(folder => {
    for (const paper of (Array.isArray(folder.papers) ? folder.papers : []).slice(0, 50)) {
      const id = paper.arxiv_id ?? paper.id;
      add(paper.url ?? `https://arxiv.org/abs/${id}`, paper.title, '', paper.publication_date ?? paper.published_at);
    }
    return { name: String(folder.name ?? '').slice(0, 100), paperCount: Number.isInteger(folder.paper_count) ? folder.paper_count : 0,
      truncated: folder.papers_truncated === true };
  });
  return { type: 'complete', sources, folders, private: true,
    warning: '文献库仅供本账号使用。仅支持读取有公开 arXiv 标识的论文；私人上传、截断列表可能未列出。' };
}
export const ALPHAXIV_PROFILE = Object.freeze({ endpoint: ALPHAXIV_ENDPOINT, client: 'memhot-alphaxiv',
  bodyLimit: 2 * 1024 * 1024, timeout: 45_000, parse: parseAlphaResult });
export async function runAlpha(start, channel, signal) {
  return runReadonly(start, channel, signal, ALPHAXIV_PROFILE);
}
