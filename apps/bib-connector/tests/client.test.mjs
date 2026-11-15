import { test } from 'node:test';
import assert from 'node:assert/strict';
import { gatewayOrigin, captureRequest, api } from '../client.js';
test('gateway credentials are restricted to explicit HTTPS or loopback origins', () => {
  assert.equal(gatewayOrigin('http://127.0.0.1:8765'), 'http://127.0.0.1:8765');
  assert.equal(gatewayOrigin('https://library.example/'), 'https://library.example');
  for (const url of ['http://example.org', 'https://u:p@example.org', 'https://example.org/path', 'https://example.org/?token=x', 'file:///tmp/a']) assert.throws(() => gatewayOrigin(url));
});
test('capture rejects blank title and limits request to metadata fields', () => {
  assert.throws(() => captureRequest({title:' '}));
  const request = captureRequest({title:' Paper ', authors:['Ada'], doi:'10.1000/x', source_url:'https://publisher.example/paper#section', token:'secret'});
  assert.equal(request.title, 'Paper');
  assert.deepEqual(request.authors, [{last_name:'Ada',fore_name:null}]);
  assert.deepEqual(request.identifiers,[{kind:'doi',value:'10.1000/x'},{kind:'other',value:'https://publisher.example/paper'}]);
  assert.deepEqual(request.pub_types,['Journal Article']);
  assert.equal(request.token,undefined);
  assert.throws(() => captureRequest({title:'x',source_url:'javascript:alert(1)'}));
  assert.throws(() => captureRequest({title:'x',source_url:'https://example.org',year:2026.5}));
});
test('API uses bearer auth without cookies and refuses redirects', async () => {
  const original = global.fetch;
  global.fetch = async (url, options) => {
    assert.equal(url,'https://library.example/api/v1/bib/collections');
    assert.equal(options.headers.Authorization,'Bearer session-token');
    assert.equal(options.credentials,'omit'); assert.equal(options.redirect,'error');
    return {ok:false,status:401,json:async()=>({error:'Invalid token'})};
  };
  try { await assert.rejects(api('https://library.example','session-token','GET','/collections'), /Invalid token/); } finally { global.fetch=original; }
});
