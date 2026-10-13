import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';

// Optional PRIVATE offline regression. No database data is saved to the repository or printed.
// This fixture was captured before opaque checkpoints; its two section keys need QA-only repair.
test('recorded four-turn QA checkpoint reproduces native history with explicit JSON output control', {
  skip: !process.env.MEMHOT_AGENT_QA_DB,
}, async () => {
  const extracted = spawnSync('python3', ['-c', `
import json,sqlite3,sys
c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
rows=[json.loads(raw) for (raw,) in c.execute('select data from public_agent_runs')]
d=next(row for row in rows if row['input']['mode']=='digest' and row['step']==4 and row['pending_models'])
if 'messages' in d:
    messages=d['messages']
    for message in messages:
        if message['role']=='system' and message.get('sections'):
            assert set(message['sections'])=={'preamble','cwd'}
            message['sections']={key:message['sections'][key] for key in ['preamble','cwd']}
    state=json.dumps(messages,ensure_ascii=False,separators=(',',':'))
else:
    state=d['messages_json']
print(json.dumps({'start':dict(d['input'],maxSteps=2,prompt='',messagesJson=state),'request':d['pending_models'][0]['request']},ensure_ascii=False))
`, process.env.MEMHOT_AGENT_QA_DB], { encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 });
  assert.equal(extracted.status, 0, 'Private QA fixture could not be loaded.');
  const fixture = JSON.parse(extracted.stdout);
  // This historical paid request predates the native JSON-mode option; history stays exact.
  fixture.request.response_format = { type: 'json_object' };
  const child = spawn(process.execPath, ['src/main.mjs'], {
    cwd: new URL('..', import.meta.url), env: { PATH: process.env.PATH }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 90_000);
  const lines = createInterface({ input: child.stdout });
  const frame = await new Promise((resolve, reject) => {
    lines.once('line', line => { try { resolve(JSON.parse(line)); } catch { reject(new Error('Invalid private replay frame.')); } });
    child.once('close', () => reject(new Error('Private replay ended before a model request.')));
    child.stdin.write(`${JSON.stringify(fixture.start)}\n`);
  });
  child.kill();
  await new Promise(resolve => child.once('close', resolve));
  clearTimeout(timer); lines.close();
  const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object'
    ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
  assert.equal(frame.type, 'model');
  assert.equal(JSON.stringify(canonical(frame.body)) === JSON.stringify(canonical(fixture.request)), true,
    'Recorded provider request differs; private payload omitted.');
});
