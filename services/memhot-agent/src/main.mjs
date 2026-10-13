import { JsonlChannel } from './protocol.mjs';
import { validateStart, LIMITS } from './validation.mjs';
import { runAgent } from './runner.mjs';
import { failure, HarnessError } from './errors.mjs';

const channel = new JsonlChannel(process.stdin, process.stdout);
const startupTimer = setTimeout(() => channel.fail(new HarnessError('RPC_TIMEOUT', 'The host did not start the agent.')), LIMITS.rpcTimeoutMs);
try {
  const input = await channel.start;
  clearTimeout(startupTimer);
  const start = validateStart(input);
  const result = await runAgent(start, channel);
  await channel.emit(result, true);
} catch (error) {
  process.exitCode = 1;
  try { await channel.emit(failure(channel.error ?? error), true); } catch { /* Host pipe may already be closed. */ }
} finally {
  clearTimeout(startupTimer);
  channel.close();
}
