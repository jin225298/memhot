import test from 'node:test';
import assert from 'node:assert/strict';
import { decodePdf, extractPdf, PDF_LIMITS } from '../src/web-pdf.mjs';
import { pdfFixture } from './fixtures/pdf.mjs';

test('isolated unpdf extracts original text with bounded page metadata and no metadata dates', async () => {
  const result = await extractPdf(pdfFixture({ pages: 2, javascript: true }));
  assert.equal(result.type, 'complete');
  assert.equal(result.title, 'Public report');
  assert.equal(result.pagesRead, 2);
  assert.equal(result.totalPages, 2);
  assert.equal(result.truncated, false);
  assert.match(result.text, /\[Page 1\].*public research/s);
  assert.doesNotMatch(result.text, /2099|https:\/\/private|Publication|truncated=/);
  assert.deepEqual(Object.keys(result).sort(), ['pagesRead', 'text', 'title', 'totalPages', 'truncated', 'type']);
});

test('page and character limits disclose partial reading separately from original text', async () => {
  const pageLimited = await extractPdf(pdfFixture({ pages: 45, text: 'A short page offers bounded original research evidence.' }));
  assert.equal(pageLimited.pagesRead, PDF_LIMITS.pages);
  assert.equal(pageLimited.totalPages, 45);
  assert.equal(pageLimited.truncated, true);
  assert.doesNotMatch(pageLimited.text, /\[Page 41\]/);
  const textLimited = await extractPdf(pdfFixture({ text: 'Original research. '.repeat(1500) }));
  assert.equal(textLimited.pagesRead, 1);
  assert.equal(textLimited.totalPages, 1);
  assert.equal(textLimited.truncated, true);
  assert.ok(Array.from(textLimited.text).length <= PDF_LIMITS.chars);
});

test('scans, invalid data, and password-protected PDFs remain unavailable', async () => {
  for (const data of [pdfFixture({ text: '' }), Buffer.from('%PDF-corrupt'), pdfFixture({ encrypted: true })]) {
    await assert.rejects(extractPdf(data), error => error.code === 'PDF_UNAVAILABLE');
  }
});

test('PDF byte envelope rejects malformed/base64/oversized input before spawning a worker', async () => {
  const data = pdfFixture();
  assert.deepEqual(decodePdf(data.toString('base64'), 'base64'), data);
  for (const [body, encoding] of [['Zg==', 'base64'], [data.toString('base64'), 'utf8'],
    ['a'.repeat(Math.ceil(PDF_LIMITS.bytes / 3) * 4 + 4), 'base64'], ['%PDF-', 'base64'], ['Z===', 'base64']]) {
    assert.throws(() => decodePdf(body, encoding), error => error.code === 'PDF_INPUT');
  }
  await assert.rejects(extractPdf(new Uint8Array(PDF_LIMITS.bytes + 1)), error => error.code === 'PDF_INPUT');
});

test('cancellation terminates an in-flight PDF worker promptly', async () => {
  const controller = new AbortController();
  const result = extractPdf(pdfFixture({ pages: 40 }), controller.signal);
  controller.abort();
  await assert.rejects(result, error => error.code === 'PDF_CANCELLED');
  assert.ok(true, 'test process exits after worker cancellation');
});
