import { runReadonly } from './mcp-readonly.mjs';
import { HarnessError } from './errors.mjs';
import { isObject, LIMITS } from './validation.mjs';

export const AIHOT_ENDPOINT = 'https://aihot.news/api/mcp';
export const AIHOT_TOOLS = Object.freeze(['aihot_get_latest', 'aihot_search', 'aihot_get_hot_topics',
  'aihot_get_story', 'aihot_get_daily', 'aihot_get_weekly', 'aihot_get_monthly']);
export const AIHOT_LIMITS = Object.freeze({ ...LIMITS, inputFrame: 1024 * 1024,
  outputFrame: 128 * 1024, totalOutput: 2 * 1024 * 1024, rpcTimeoutMs: 30_000 });
const failure = code => new HarnessError(code, 'AIHOT public discovery is unavailable.');

export function validateAiHotStart(value) {
  if (!isObject(value) || value.type !== 'start' || !AIHOT_TOOLS.includes(value.tool) ||
    !isObject(value.args) || Buffer.byteLength(JSON.stringify(value.args)) > 4096 ||
    Object.keys(value).some(key => !['type', 'tool', 'args'].includes(key))) throw failure('INVALID_START');
  return value;
}

function publicUrl(value, aihot = false) {
  if (typeof value !== 'string' || value.length > 2048) return undefined;
  try {
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password ||
      (aihot && (url.origin !== 'https://aihot.news' || url.search || url.hash))) return undefined;
    return url.toString();
  } catch { return undefined; }
}

/** Summaries remain discoveries. Only the host's original-page reader can admit evidence. */
export function parseAiHotResult(result, tool) {
  if (!isObject(result) || result.isError || !isObject(result.structuredContent) ||
      result.structuredContent.schemaVersion !== 1) throw failure('AIHOT_RESPONSE');
  const data = result.structuredContent;
  const sources = [], stories = [], seen = new Set();
  let nodes = 0;
  const visit = (value, depth = 0) => {
    if (++nodes > 512 || depth > 10) throw failure('AIHOT_RESPONSE');
    if (Array.isArray(value)) { for (const child of value.slice(0, 50)) visit(child, depth + 1); return; }
    if (!isObject(value)) return;
    const links = value.links;
    if (isObject(links)) {
      const original = publicUrl(links.original);
      const attribution = publicUrl(value.attribution?.url ?? links.aihot ?? data.report?.attribution?.url, true);
      if (original && attribution && typeof value.title === 'string' && !seen.has(original) && sources.length < 20) {
        seen.add(original);
        sources.push({ title: value.title.slice(0, 200), url: original,
          text: typeof value.summary === 'string' ? value.summary.slice(0, 1200) : '',
          attribution: { name: 'AIHOT', url: attribution }, fulltext: false });
      }
      const story = publicUrl(links.story, true);
      if (tool === 'aihot_get_hot_topics' && story) {
        const id = new URL(story).pathname.split('/').filter(Boolean).at(-1);
        if (/^[A-Za-z0-9_-]{1,128}$/.test(id ?? '') && stories.length < 10) stories.push({ id, url: story });
      }
    }
    for (const key of ['items', 'topics', 'report', 'sections', 'flashes', 'story', 'timeline',
      'reports', 'relatedEvents', 'related', 'entries']) if (value[key] !== undefined) visit(value[key], depth + 1);
  };
  visit(data);
  const attribution = publicUrl(data.report?.attribution?.url ?? data.report?.links?.aihot, true) ?? 'https://aihot.news/';
  const report = data.report;
  const text = (value, max) => typeof value === 'string' ? value.slice(0, max) : '';
  const overview = isObject(report) ? {
    attribution: { name: 'AIHOT', url: attribution },
    headline: text(report.lead?.title ?? report.headline, 200),
    summary: text(report.lead?.leadParagraph ?? report.overview, 1000),
    period: Object.fromEntries(['date', 'week', 'month', 'periodStart', 'periodEnd', 'windowStart', 'windowEnd']
      .filter(key => typeof report[key] === 'string').map(key => [key, text(report[key], 64)])),
    sections: (Array.isArray(report.sections) ? report.sections : []).slice(0, 6).map(section => ({
      label: text(section?.label, 80), summary: text(section?.summary, 160),
      titles: (Array.isArray(section?.items) ? section.items : []).slice(0, 4).map(item => text(item?.title, 80)),
    })),
    warning: 'AIHOT 编辑概览仅作发现背景，不是已核对原文证据，不能据此生成事实引文。',
  } : undefined;
  return { type: 'complete', sources, stories, attribution: { name: 'AIHOT', url: attribution },
    ...(overview ? { overview } : {}),
    warning: 'AIHOT 聚合摘要仅作发现；请读取原始链接后再引用事实，不代表已读原文。' };
}

export const AIHOT_PROFILE = Object.freeze({ endpoint: AIHOT_ENDPOINT, client: 'memhot-aihot',
  bodyLimit: 512 * 1024, timeout: 15_000, parse: (result, start) => parseAiHotResult(result, start.tool) });
export async function runAiHot(start, channel, signal) {
  return runReadonly(start, channel, signal, AIHOT_PROFILE);
}
