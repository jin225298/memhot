import { JsonlChannel } from './protocol.mjs';
import { HarnessError, failure } from './errors.mjs';
import { runWeb, validateWebStart, WEB_LIMITS } from './web-worker.mjs';

// Third-party diagnostics must never enter stdout JSONL or expose page/provider contents on stderr.
for (const name of ['log', 'info', 'debug', 'warn', 'error']) console[name] = () => {};
const channel = new JsonlChannel(process.stdin, process.stdout, WEB_LIMITS);
const controller = new AbortController();
const timer = setTimeout(() => {
  channel.fail(new HarnessError('RUN_TIMEOUT', 'The web tool timed out.'));
  controller.abort();
}, WEB_LIMITS.rpcTimeoutMs);
try {
  const start = validateWebStart(await channel.start);
  const result = await runWeb(start, channel, controller.signal);
  await channel.emit(result, true);
} catch (error) {
  process.exitCode = 1;
  try { await channel.emit(failure(channel.error ?? error), true); } catch { /* The host may have closed its pipe. */ }
} finally {
  clearTimeout(timer);
  controller.abort();
  channel.close();
}
