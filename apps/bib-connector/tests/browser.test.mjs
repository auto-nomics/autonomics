import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extractMetadata } from '../extract.js';
const require = createRequire(new URL('../../gui/package.json', import.meta.url));
const { chromium } = require('playwright');
const executablePath = process.env.PDF_TEST_CHROMIUM || ['/usr/bin/chromium', '/opt/google/chrome/chrome'].find(existsSync);

test('publisher metadata and generic pages are captured in a real DOM', async () => {
  const browser = await chromium.launch({executablePath,args:['--no-sandbox']});
  try {
    const page = await browser.newPage();
    await page.route('https://publisher.example/paper', route => route.fulfill({contentType:'text/html',body:`
      <title>Publisher branding</title><meta name="citation_title" content="A scientific paper">
      <meta name="citation_author" content="Wang, Ming"><meta name="citation_author" content="Ada Lovelace">
      <meta name="citation_doi" content="https://doi.org/10.1234/example"><meta name="citation_date" content="2026/01/02">
      <meta name="citation_journal_title" content="Journal"><meta name="citation_keywords" content="genetics; science">
      <script type="application/ld+json">broken</script>`}));
    await page.goto('https://publisher.example/paper');
    const captured = await page.evaluate(extractMetadata);
    assert.equal(captured.title,'A scientific paper'); assert.equal(captured.doi,'10.1234/example');
    assert.equal(captured.year,2026); assert.deepEqual(captured.authors,['Wang, Ming','Ada Lovelace']);
    assert.deepEqual(captured.keywords,['genetics','science']);
    await page.setContent(`<title>Fallback</title><script type="application/ld+json">{"@graph":[{"@type":"ScholarlyArticle","headline":"JSON-LD title","author":[{"name":"Ada"}],"datePublished":"2025-05-01"}]}</script>`);
    const json = await page.evaluate(extractMetadata);
    assert.equal(json.title,'JSON-LD title'); assert.deepEqual(json.authors,['Ada']); assert.equal(json.year,2025);
    await page.setContent('<title>A plain webpage</title><meta name="description" content="A summary">');
    const generic = await page.evaluate(extractMetadata);
    assert.equal(generic.title,'A plain webpage'); assert.equal(generic.abstract_text,'A summary'); assert.equal(generic.doi,'');
  } finally { await browser.close(); }
});

test('popup preview, session credentials, save, collection and persistent outbox retry', async () => {
  const server = createServer(async (req,res) => {
    const name = new URL(req.url,'http://localhost').pathname.slice(1) || 'popup.html';
    if (!['popup.html','popup.js','style.css','extract.js','client.js'].includes(name)) {res.writeHead(404).end();return;}
    res.setHeader('Content-Type',name.endsWith('.js')?'application/javascript':name.endsWith('.css')?'text/css':'text/html');
    res.end(await readFile(new URL(`../${name}`,import.meta.url)));
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const browser = await chromium.launch({executablePath,args:['--no-sandbox']});
  try {
    const page = await browser.newPage();
    await page.addInitScript(() => {
      window.localData = {}; window.sessionData = {}; window.permissions = [];
      const store = data => ({get:async keys=>Object.fromEntries((Array.isArray(keys)?keys:[keys]).map(key=>[key,data[key]])),set:async value=>Object.assign(data,value),remove:async key=>delete data[key],setAccessLevel:async()=>{}});
      window.chrome = {storage:{local:store(window.localData),session:store(window.sessionData)}, permissions:{request:async request=>{window.permissions.push(request);return true;}},tabs:{query:async()=>[{id:1}]},scripting:{executeScript:async()=>[{result:{title:'Captured paper',authors:['Ada'],doi:'10.1000/example',source_url:'https://publisher.example/paper',keywords:[]}}]}};
    });
    const requests=[];let fail=false;
    await page.route('http://127.0.0.1:8765/api/v1/bib/**', async route=>{
      const req=route.request();requests.push({url:req.url(),method:req.method(),body:req.postDataJSON(),auth:req.headers().authorization});
      const headers={'Access-Control-Allow-Origin':'*','Access-Control-Allow-Headers':'Authorization,Content-Type','Access-Control-Allow-Methods':'GET,POST,OPTIONS'};
      if(req.method()==='OPTIONS')return route.fulfill({status:204,headers});
      if(req.url().endsWith('/collections'))return route.fulfill({headers,json:{collections:[{id:'c1',name:'Research'}]}});
      if(req.url().endsWith('/articles/manual'))return route.fulfill({headers,status:fail?503:201,json:fail?{error:'Try later'}:{article:{id:'saved'}}});
      return route.fulfill({headers,json:{ok:true}});
    });
    await page.goto(`http://127.0.0.1:${server.address().port}/popup.html`);
    await page.waitForFunction(()=>document.getElementById('title').value==='Captured paper');
    await page.locator('#token').fill('private-token');await page.getByRole('button',{name:'Connect',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Connected'}).waitFor();
    assert.equal((await page.evaluate(()=>window.localData)).token,undefined);
    assert.equal((await page.evaluate(()=>window.sessionData)).token,'private-token');
    await page.locator('#collection').selectOption('c1');
    await page.locator('#title').fill('Reviewed title');
    await page.getByRole('button',{name:'Save metadata',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Saved to your library'}).waitFor();
    const saved=requests.find(req=>req.url.endsWith('/articles/manual')&&req.method==='POST');
    assert.equal(saved.body.title,'Reviewed title');assert.equal(saved.auth,'Bearer private-token');
    assert.equal(saved.body.identifiers[1].value,'https://publisher.example/paper');
    assert.ok(requests.some(req=>req.url.endsWith('/collections/c1/articles')));
    assert.deepEqual((await page.evaluate(()=>window.localData)).outbox,[]);
    fail=true;await page.getByRole('button',{name:'Save metadata',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Try later'}).waitFor();
    assert.equal((await page.evaluate(()=>window.localData)).outbox.length,1);
    await page.locator('#origin').fill('https://different.example');
    await page.getByRole('button',{name:'Retry',exact:true}).click();
    await page.getByRole('status').filter({hasText:'belongs to'}).waitFor();
    fail=false;await page.locator('#origin').fill('http://127.0.0.1:8765');
    await page.getByRole('button',{name:'Retry',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Saved to your library'}).waitFor();
    await page.getByRole('button',{name:'Forget token',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Token forgotten'}).waitFor();
    assert.equal((await page.evaluate(()=>window.sessionData)).token,undefined);
  } finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
});
