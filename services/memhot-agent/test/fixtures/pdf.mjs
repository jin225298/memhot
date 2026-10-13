// Minimal valid, text-bearing PDF. No package download or binary fixture needed.
export function pdfFixture({ pages = 1, text = 'A public research report presents source evidence and limitations. '.repeat(4),
  javascript = false, encrypted = false, padding = 0, title = 'Public report' } = {}) {
  const objects = ['<< /Type /Catalog /Pages 2 0 R' + (javascript ? ' /OpenAction << /S /JavaScript /JS (fetch\\(https://private.invalid/\\)) >>' : '') + ' >>', '',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>', `<< /Title (${title}) /CreationDate (D:20990101000000Z) /ModDate (D:20990102000000Z) >>`];
  const kids = [];
  for (let page = 0; page < pages; page++) {
    const id = objects.length + 1;
    kids.push(`${id} 0 R`);
    objects.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1000000 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${id + 1} 0 R >>`);
    const stream = `BT /F1 12 Tf 72 720 Td (${text.replace(/[\\()]/g, '\\$&')}) Tj ET`;
    objects.push(`<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`);
  }
  objects[1] = `<< /Type /Pages /Count ${pages} /Kids [${kids.join(' ')}] >>`;
  if (encrypted) objects.push('<< /Filter /Standard /V 1 /R 2 /Length 40 /O (protected) /U (protected) /P -4 >>');
  let data = '%PDF-1.4\n' + (padding ? '%' + 'x'.repeat(padding) + '\n' : '');
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(data)); data += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const start = Buffer.byteLength(data);
  data += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  data += offsets.slice(1).map(offset => `${String(offset).padStart(10, '0')} 00000 n \n`).join('');
  data += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R /Info 4 0 R${encrypted ? ` /Encrypt ${objects.length} 0 R` : ''} >>\nstartxref\n${start}\n%%EOF\n`;
  return Buffer.from(data);
}
