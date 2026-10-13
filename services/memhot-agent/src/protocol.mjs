import { TextDecoder } from 'node:util';
import { HarnessError } from './errors.mjs';
import { isObject, LIMITS } from './validation.mjs';

/** One process, one start frame, correlated host RPCs, one terminal frame. */
export class JsonlChannel {
  constructor(input, output, limits = LIMITS) {
    this.input = input;
    this.output = output;
    this.limits = limits;
    this.buffer = Buffer.alloc(0);
    this.pending = new Map();
    this.cancelled = new Set();
    this.counter = 0;
    this.written = 0;
    this.closed = false;
    this.started = false;
    this.start = new Promise((resolve, reject) => {
      this.resolveStart = resolve;
      this.rejectStart = reject;
    });
    this.onData = chunk => this.receive(chunk);
    this.onEnd = () => this.fail(new HarnessError('RPC_CLOSED', 'The host connection closed.'));
    this.onError = () => this.fail(new HarnessError('RPC_CLOSED', 'The host connection failed.'));
    input.on('data', this.onData);
    input.on('end', this.onEnd);
    input.on('error', this.onError);
    output.on('error', this.onError);
  }

  receive(chunk) {
    if (this.closed || this.error) return;
    const incoming = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    this.buffer = Buffer.concat([this.buffer, incoming]);
    try {
      let offset;
      while ((offset = this.buffer.indexOf(10)) !== -1) {
        if (offset > this.limits.inputFrame) throw new HarnessError('INPUT_LIMIT', 'A host frame exceeded the input limit.');
        const line = this.buffer.subarray(0, offset);
        this.buffer = this.buffer.subarray(offset + 1);
        const text = new TextDecoder('utf-8', { fatal: true }).decode(line);
        let frame;
        try { frame = JSON.parse(text); } catch { throw new HarnessError('RPC_PROTOCOL', 'The host sent an invalid JSON frame.'); }
        if (!this.started) {
          this.started = true;
          this.resolveStart(frame);
        } else this.result(frame);
      }
      if (this.buffer.length > this.limits.inputFrame) throw new HarnessError('INPUT_LIMIT', 'A host frame exceeded the input limit.');
    } catch (error) {
      this.fail(error instanceof HarnessError ? error : new HarnessError('RPC_PROTOCOL', 'The host sent an invalid frame.'));
    }
  }

  result(frame) {
    if (!isObject(frame) || !['result', 'error'].includes(frame.type) || typeof frame.id !== 'string') {
      throw new HarnessError('RPC_PROTOCOL', 'The host sent an unexpected RPC response.');
    }
    // A cancelled cleanup RPC can finish after the useful MCP result. Ignore it once.
    if (this.cancelled.delete(frame.id)) return;
    if (!this.pending.has(frame.id)) throw new HarnessError('RPC_PROTOCOL', 'The host sent an unexpected RPC response.');
    const pending = this.pending.get(frame.id);
    this.pending.delete(frame.id);
    pending.cleanup();
    if (frame.type === 'error') {
      // Host/provider error text can contain credentials or raw response contents.
      pending.reject(new HarnessError(`${pending.kind.toUpperCase()}_FAILED`, `The host ${pending.kind} request failed.`));
    } else if (!Object.hasOwn(frame, 'value')) {
      pending.reject(new HarnessError('RPC_PROTOCOL', 'The host RPC response has no result.'));
    } else pending.resolve(frame.value);
  }

  async emit(frame, terminal = false) {
    if (this.closed) throw new HarnessError('RPC_CLOSED', 'The host connection closed.');
    const line = `${JSON.stringify(frame)}\n`;
    const size = Buffer.byteLength(line);
    if (size > this.limits.outputFrame || (!(terminal && frame.type === 'failed') && this.written + size > this.limits.totalOutput)) {
      throw new HarnessError('OUTPUT_LIMIT', 'The agent exceeded its output limit.');
    }
    this.written += size;
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new HarnessError('RPC_TIMEOUT', 'The host output timed out.')), this.limits.rpcTimeoutMs);
      this.output.write(line, error => {
        clearTimeout(timer);
        if (error) reject(new HarnessError('RPC_CLOSED', 'The host connection failed.'));
        else resolve();
      });
    });
  }

  async request(kind, payload, signal, { isolatedCancellation = false } = {}) {
    if (this.error) throw this.error;
    if (this.closed) throw new HarnessError('RPC_CLOSED', 'The host connection closed.');
    if (signal?.aborted) throw new HarnessError('RUN_TIMEOUT', 'The agent run was interrupted.');
    if (this.pending.size >= 16) throw new HarnessError('RPC_LIMIT', 'Too many host requests are pending.');
    const id = `rpc-${++this.counter}`;
    let rejectPending;
    const response = new Promise((resolve, reject) => {
      rejectPending = reject;
      const abort = () => {
        const error = new HarnessError('RUN_TIMEOUT', 'The agent run was interrupted.');
        if (!isolatedCancellation || this.cancelled.size >= 16) { this.fail(error); return; }
        this.pending.get(id)?.cleanup();
        this.pending.delete(id);
        this.cancelled.add(id);
        reject(error);
      };
      const timer = setTimeout(() => this.fail(new HarnessError('RPC_TIMEOUT', 'The host request timed out.')), this.limits.rpcTimeoutMs);
      const cleanup = () => { clearTimeout(timer); signal?.removeEventListener('abort', abort); };
      this.pending.set(id, { resolve, reject, cleanup, kind });
      signal?.addEventListener('abort', abort, { once: true });
    });
    // Attach rejection handling before an emit failure or synchronous input failure.
    response.catch(() => {});
    try { await this.emit({ type: kind, id, ...payload }); } catch (error) {
      this.pending.get(id)?.cleanup();
      this.pending.delete(id);
      rejectPending(error);
    }
    return response;
  }

  fail(error) {
    if (this.error || this.closed) return;
    this.error = error;
    this.rejectStart(error);
    for (const pending of this.pending.values()) {
      pending.cleanup();
      pending.reject(error);
    }
    this.pending.clear();
  }

  close() {
    if (this.closed) return;
    this.fail(new HarnessError('RPC_CLOSED', 'The host connection closed.'));
    this.closed = true;
    this.input.off('data', this.onData);
    this.input.off('end', this.onEnd);
    this.input.off('error', this.onError);
    this.output.off('error', this.onError);
    this.input.pause();
  }
}
