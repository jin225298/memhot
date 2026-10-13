import { JsonlChannel } from './protocol.mjs';
import { HarnessError, failure } from './errors.mjs';
import { ALPHAXIV_LIMITS, runAlpha, validateAlphaStart } from './alphaxiv-worker.mjs';

for (const name of ['log', 'info', 'debug', 'warn', 'error']) console[name] = () => {};
// The sole transport is explicitly injected into Pi MCP; reject accidental direct fetches.
globalThis.fetch = async () => { throw new HarnessError('NETWORK_FORBIDDEN', 'Direct network is unavailable.'); };
for (const key of Object.keys(process.env)) delete process.env[key];
const channel = new JsonlChannel(process.stdin, process.stdout, ALPHAXIV_LIMITS);
const controller = new AbortController();
const timer = setTimeout(() => {
  channel.fail(new HarnessError('RUN_TIMEOUT', 'Paper retrieval timed out.'));
  controller.abort();
}, ALPHAXIV_LIMITS.rpcTimeoutMs);
try {
  const start = validateAlphaStart(await channel.start);
  await channel.emit(await runAlpha(start, channel, controller.signal), true);
} catch (error) {
  process.exitCode = 1;
  try { await channel.emit(failure(channel.error ?? error), true); } catch { /* Closed host pipe. */ }
} finally {
  clearTimeout(timer);
  controller.abort();
  channel.close();
}
