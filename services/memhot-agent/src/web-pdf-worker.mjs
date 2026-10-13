// unpdf's bundled PDF.js parses data only. It does not render, run PDF actions,
// OCR, follow links, submit forms, or request passwords.
import { parentPort } from 'node:worker_threads';
import { getDocumentProxy } from 'unpdf';
import { syncBuiltinESMExports } from 'node:module';
import net from 'node:net';
import tls from 'node:tls';
import http from 'node:http';
import https from 'node:https';
import http2 from 'node:http2';
import dgram from 'node:dgram';
import dns from 'node:dns';
import dnsPromises from 'node:dns/promises';

const deny = () => { throw new Error('PDF resources are unavailable.'); };
for (const key of Object.keys(process.env)) delete process.env[key];
globalThis.fetch = deny; globalThis.WebSocket = deny;
for (const name of ['log', 'info', 'debug', 'warn', 'error']) console[name] = () => {};
for (const [module, names] of [[net, ['connect', 'createConnection', 'createServer']],
  [tls, ['connect', 'createServer']], [http, ['request', 'get', 'createServer']],
  [https, ['request', 'get', 'createServer']], [http2, ['connect', 'createServer', 'createSecureServer']],
  [dgram, ['createSocket']]]) for (const name of names) module[name] = deny;
net.Socket.prototype.connect = deny;
for (const module of [dns, dnsPromises]) {
  for (const name of Object.keys(module)) {
    if (name === 'lookup' || name.startsWith('resolve') || name === 'reverse') module[name] = deny;
  }
  for (const name of Object.getOwnPropertyNames(module.Resolver.prototype)) {
    if (name.startsWith('resolve') || name === 'reverse') module.Resolver.prototype[name] = deny;
  }
}
syncBuiltinESMExports();
class NoExternalResources { fetch() { return Promise.reject(new Error('PDF resources are unavailable.')); } }

parentPort.once('message', async ({ data, limits }) => {
  let pdf;
  try {
    if (!(data instanceof Uint8Array) || data.byteLength > limits.bytes) throw new Error('limit');
    pdf = await getDocumentProxy(data, { verbosity: 0, useWorkerFetch: false, useWasm: false, isEvalSupported: false,
      BinaryDataFactory: NoExternalResources, useSystemFonts: false, disableFontFace: true,
      cMapUrl: undefined, standardFontDataUrl: undefined, wasmUrl: undefined, iccUrl: undefined,
      enableXfa: false, isOffscreenCanvasSupported: false, isImageDecoderSupported: false,
      disableRange: true, disableStream: true, disableAutoFetch: true, stopAtErrors: true,
      maxImageSize: 1, canvasMaxAreaInBytes: 0 });
    const { info } = await pdf.getMetadata();
    if (info?.EncryptFilterName) throw new Error('protected');
    let text = '', chars = 0, pagesRead = 0, truncated = false;
    for (let number = 1; number <= Math.min(pdf.numPages, limits.pages); number++) {
      const page = await pdf.getPage(number);
      const reader = page.streamTextContent().getReader();
      pagesRead = number;
      try {
        const heading = `\n[Page ${number}]\n`;
        if (chars + heading.length >= limits.chars) { truncated = true; break; }
        text += heading; chars += heading.length;
        while (chars < limits.chars) {
          const chunk = await reader.read();
          if (chunk.done) break;
          for (const item of chunk.value.items) {
            if (typeof item.str !== 'string') continue;
            const remaining = limits.chars - chars;
            const original = item.str + (item.hasEOL ? '\n' : ' ');
            const points = Array.from(original.slice(0, remaining * 2));
            const part = points.slice(0, remaining).join('');
            text += part; chars += Math.min(points.length, remaining);
            if (points.length > remaining || original.length > remaining * 2 || chars === limits.chars) {
              truncated = true; break;
            }
          }
          if (truncated) break;
        }
      } finally { await reader.cancel(new Error('PDF extraction completed.')).catch(() => {}); reader.releaseLock(); page.cleanup(); }
      if (truncated) break;
    }
    if (pagesRead < pdf.numPages) truncated = true;
    // Page markers alone are not usable text; scanned and empty PDFs stay unavailable.
    if (text.replace(/\[Page \d+\]/g, '').trim().length < 100) throw new Error('no text');
    const title = typeof info?.Title === 'string' ? info.Title.replace(/[\u0000-\u001f]/g, ' ').trim().slice(0, 1024) : '';
    parentPort.postMessage({ type: 'complete', text: text.trim(), title, pagesRead, totalPages: pdf.numPages, truncated });
  } catch { parentPort.postMessage({ type: 'failed' }); }
  finally { await pdf?.loadingTask.destroy().catch(() => {}); parentPort.close(); }
});
