import test from 'node:test';
import assert from 'node:assert/strict';
import { ALPHAXIV_ENDPOINT, ALPHAXIV_PROFILE, runAlpha, validateAlphaStart, parseAlphaResult, paperUrl } from '../src/alphaxiv-worker.mjs';
import { rpcFetch } from '../src/mcp-readonly.mjs';
const start = { type:'start', tool:'get_paper_content', args:{url:'https://arxiv.org/pdf/2601.12345',fullText:true} };
test('paper URLs and raw-text enforcement reject writes, arbitrary hosts and generated reports', () => {
  for(const url of ['https://arxiv.org.evil/abs/2601.12345','https://arxiv.org/abs/2601.12345?q=private','https://www.alphaxiv.org/settings/api-keys','http://arxiv.org/abs/2601.12345']) assert.equal(paperUrl(url),undefined);
  assert.equal(paperUrl('https://www.alphaxiv.org/abs/2601.12345v2'),'https://arxiv.org/pdf/2601.12345v2');
  assert.equal(paperUrl('https://arxiv.org/pdf/hep-th/9901001.pdf'),'https://arxiv.org/pdf/hep-th/9901001');
  for(const bad of [{...start,tool:'delete_folder'},{...start,apiKey:'secret'},{...start,args:{...start.args,fullText:false}}]) assert.throws(()=>validateAlphaStart(bad));
});
test('live-format discovery is bounded and preserves original dates without claiming fulltext', () => {
  const response=parseAlphaResult({content:[{type:'text',text:'1. [ID=2601.12345] **Synthetic paper** (https://www.alphaxiv.org/abs/2601.12345). Published 2026-01-08 by Test: An abstract.'}]},{tool:'discover_papers'});
  assert.equal(response.sources[0].publishedAt,'2026-01-08'); assert.equal(response.sources[0].url,'https://arxiv.org/pdf/2601.12345');
  assert.equal(response.sources[0].fulltext,undefined);
  assert.throws(()=>parseAlphaResult({content:[{type:'text',text:'Changed structure https://arxiv.org/abs/2601.12345'}]},{tool:'discover_papers'}));
});
test('library filters private uploads and malformed URLs and keeps folders private', () => {
  const response=parseAlphaResult({content:[{type:'text',text:JSON.stringify({folders:[{name:'Reading',paper_count:2,papers:[{id:'2601.12345',title:'Paper'},{id:'private-uuid',title:'Upload'}]}]})}]},{tool:'list_library'});
  assert.equal(response.private,true); assert.equal(response.sources.length,1); assert.equal(response.folders[0].paperCount,2);
});
test('fulltext stays raw and bounded, never silently admits remote errors', () => {
  const response=parseAlphaResult({content:[{type:'text',text:'Original paper text. '.repeat(10000)}]},start);
  assert.equal(response.fulltext,true);assert.equal(response.truncated,true);assert.equal(response.text.length,100000);
  assert.throws(()=>parseAlphaResult({isError:true,content:[]},start));
});
test('real Pi SDK uses only readonly metadata and injected fixed-endpoint RPC without credentials', async () => {
  for(const readonly of [true,false]) {
    let calls=0;
    const channel={async request(_,frame){
      assert.equal(frame.url,ALPHAXIV_ENDPOINT);assert.equal(frame.headers.authorization,undefined);
      const body=JSON.parse(frame.body??'{}');let result,status=200;
      if(frame.method==='DELETE') return {status:204,body:'',headers:{},url:ALPHAXIV_ENDPOINT,contentType:'application/json'};
      if(body.method==='initialize') result={protocolVersion:'2025-11-25',capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}};
      if(body.method==='notifications/initialized')status=202;
      if(body.method==='tools/list')result={tools:[{name:start.tool,inputSchema:{type:'object'},annotations:{readOnlyHint:readonly,destructiveHint:false}}]};
      if(body.method==='tools/call'){calls++;assert.deepEqual(body.params.arguments,start.args);result={content:[{type:'text',text:'Original evidence. '.repeat(30)}]};}
      return {status,body:result?JSON.stringify({jsonrpc:'2.0',id:body.id,result}):'',headers:{'mcp-session-id':'fixture'},url:ALPHAXIV_ENDPOINT,contentType:'application/json'};
    }};
    if(readonly) assert.equal((await runAlpha(start,channel,new AbortController().signal)).fulltext,true);
    else await assert.rejects(runAlpha(start,channel,new AbortController().signal));
    assert.equal(calls,readonly?1:0);
  }
});
test('model/child cannot send credentials or switch endpoints through shared transport', async()=>{
  let calls=0;
  for(const [url,headers] of [[ALPHAXIV_ENDPOINT,{authorization:'secret'}],['https://evil.invalid',{}]]){
    const rpc=rpcFetch(start,{request:async()=>{calls++;}},new AbortController().signal,ALPHAXIV_PROFILE);
    await assert.rejects(rpc.fetch(url,{method:'POST',headers,body:'{}'}));
  }
  assert.equal(calls,0);
});
