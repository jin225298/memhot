import { Worker } from 'node:worker_threads';
import { HarnessError } from './errors.mjs';

export const PDF_LIMITS = Object.freeze({ bytes: 8 * 1024 * 1024, pages: 40,
  chars: 17_500, timeoutMs: 15_000, heapMb: 64, outputBytes: 128 * 1024 });
const failed = (code = 'PDF_UNAVAILABLE') => new HarnessError(code,
  'The public PDF has no usable text, is protected, or exceeded the extraction limits.');

export function decodePdf(body, encoding) {
  const max = Math.ceil(PDF_LIMITS.bytes / 3) * 4;
  if (encoding !== 'base64' || typeof body !== 'string' || body.length > max ||
    body.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(body)) throw failed('PDF_INPUT');
  const data = Buffer.from(body, 'base64');
  if (data.length > PDF_LIMITS.bytes || !data.subarray(0, 5).equals(Buffer.from('%PDF-')) ||
    data.toString('base64') !== body) throw failed('PDF_INPUT');
  return data;
}

/** Only host-fetched bytes enter this worker; no URLs, credentials, or proxies. */
export async function extractPdf(data, signal) {
  if (!(data instanceof Uint8Array) || data.byteLength > PDF_LIMITS.bytes || signal?.aborted) throw failed('PDF_INPUT');
  const bytes = Uint8Array.from(data);
  const worker = new Worker(new URL('./web-pdf-worker.mjs', import.meta.url), {
    env: {}, execArgv: [], stdin: false, stdout: true, stderr: true,
    resourceLimits: { maxOldGenerationSizeMb: PDF_LIMITS.heapMb, maxYoungGenerationSizeMb: 8, stackSizeMb: 2 },
  });
  // Third-party diagnostics never become source evidence or process output.
  worker.stdout.resume(); worker.stderr.resume();
  let timer, abort;
  try {
    return await new Promise((resolve, reject) => {
      abort = () => reject(failed('PDF_CANCELLED'));
      timer = setTimeout(() => reject(failed('PDF_TIMEOUT')), PDF_LIMITS.timeoutMs);
      signal?.addEventListener('abort', abort, { once: true });
      worker.once('error', () => reject(failed()));
      worker.once('exit', () => reject(failed()));
      worker.once('message', value => {
        if (!value || value.type !== 'complete' || typeof value.text !== 'string' ||
          Buffer.byteLength(value.text) > PDF_LIMITS.outputBytes || Array.from(value.text).length > 18_000 ||
          !Number.isInteger(value.pagesRead) || value.pagesRead < 1 || value.pagesRead > PDF_LIMITS.pages ||
          !Number.isInteger(value.totalPages) || value.totalPages < value.pagesRead || typeof value.truncated !== 'boolean' ||
          typeof value.title !== 'string' || value.title.length > 1024) return reject(failed());
        resolve(value);
      });
      worker.postMessage({ data: bytes, limits: PDF_LIMITS }, [bytes.buffer]);
      if (signal?.aborted) abort();
    });
  } finally {
    clearTimeout(timer); signal?.removeEventListener('abort', abort);
    await worker.terminate();
  }
}
