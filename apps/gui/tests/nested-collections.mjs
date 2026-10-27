// Run with npm run test:bib-menu. Install a browser with npx playwright install chromium,
// or set PDF_TEST_CHROMIUM to an existing Chromium executable.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';

const collections = [
 {id:'root',name:'Research',parent_id:null,article_ids:['a1'],deleted:false},
 {id:'child',name:'Methods',parent_id:'root',article_ids:['a1'],deleted:false},
 {id:'leaf',name:'Single cell',parent_id:'child',article_ids:[],deleted:false},
];
const articles=[{id:'a1',title:'Alpha paper',authors:[],identifiers:[]}];
const requests=[];
function descendants(id) {const ids=new Set([id]);for(let i=0;i<collections.length;i++) for(const c of collections) if(ids.has(c.parent_id))ids.add(c.id);return ids;}
const origin = 'http://127.0.0.1:1425';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1425', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());
  await page.route(`${origin}/`, route => route.fulfill({ contentType: 'text/html', body: '<html lang="zh-CN"><body></body></html>' }));
  await page.route('**/api/v1/**', async route => {
    const request=route.request();const url=new URL(request.url());const path=url.pathname.replace(/^\/api\/v1/,'');const method=request.method();
    requests.push(`${method} ${path}${url.search}`);
    const body=request.postDataJSON();
    if(path==='/bib/health')return route.fulfill({json:{article_count:1}});
    if(path==='/bib/collections') {
      if(method==='POST') {const c={id:'new',name:body.name,parent_id:body.parent_id,article_ids:[],deleted:false};collections.push(c);return route.fulfill({json:{collection:c}});}
      return route.fulfill({json:{collections:structuredClone(collections)}});
    }
    const match=path.match(/^\/bib\/collections\/([^/]+)(\/restore)?$/);
    if(match){
      const c=collections.find(c=>c.id===match[1]);
      if(method==='PATCH') {if('name' in body)c.name=body.name;if('parent_id' in body)c.parent_id=body.parent_id;return route.fulfill({json:{collection:structuredClone(c)}});}
      if(method==='DELETE'){const ids=descendants(c.id);for(const node of collections)if(ids.has(node.id))node.deleted=true;return route.fulfill({json:{deleted_ids:[...ids]}});}
      if(method==='POST' && match[2]) {const ids=descendants(c.id);for(const node of collections)if(ids.has(node.id))node.deleted=false;return route.fulfill({json:{restored_ids:[...ids]}});}
    }
    if(path.endsWith('/articles') || path==='/bib/articles/unfiled') return route.fulfill({json:{articles,total:1,associations:[{article_id:'a1',collection_id:'child'}]}});
    if(path==='/bib/articles/a1') return route.fulfill({json:{article:articles[0],annotations:[]}});
    return route.fulfill({json:{}});
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    await import('/src/app.css');
    const { default: RefreshRuntime } = await import('/@react-refresh');
    RefreshRuntime.injectIntoGlobalHook(window);
    window.$RefreshReg$ = () => {};
    window.$RefreshSig$ = () => type => type;
    window.__vite_plugin_react_preamble_installed__ = true;
    const { mountBibliography } = await import('/tests/bibliography-menu-mount.tsx');
    mountBibliography(document.body);
  });
  const tree=page.getByRole('tree',{name:'Collections'});
  await tree.getByRole('treeitem',{name:'Research',exact:true}).waitFor();
  assert.equal(await tree.getByRole('treeitem',{name:'Methods',exact:true}).count(),0);
  await page.getByRole('button',{name:'Expand Research',exact:true}).click();
  await tree.getByRole('treeitem',{name:'Methods',exact:true}).click();
  await page.getByRole('button',{name:'Expand Methods',exact:true}).click();
  assert.equal(await tree.getByRole('treeitem',{name:'Single cell',exact:true}).getAttribute('aria-level'),'3');
  const recursiveResponse=page.waitForResponse(response=>response.url().includes('include_descendants=true'));
  await page.getByRole('checkbox',{name:'Show items from subcollections'}).check();
  await recursiveResponse;
  await page.waitForFunction(()=>localStorage.getItem('autonomics.collections.recursive')==='true');
  assert.ok(requests.some(r=>r.includes('include_descendants=true')));
  await tree.getByRole('treeitem',{name:'Methods',exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:'New subcollection…',exact:true}).click();
  const dialog=page.getByRole('dialog');
  await dialog.getByLabel('Name',{exact:true}).fill('Evidence');
  assert.equal(await dialog.getByLabel('Parent collection').inputValue(),'child');
  await dialog.getByRole('button',{name:'Save',exact:true}).click();
  await tree.getByRole('treeitem',{name:'Evidence',exact:true}).waitFor();
  assert.equal(collections.find(c=>c.id==='new').parent_id,'child');
  await tree.getByRole('treeitem',{name:'Evidence',exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:'Move collection…',exact:true}).click();
  assert.equal(await dialog.getByLabel('Parent collection').locator('option[value="new"]').count(),0);
  await dialog.getByLabel('Parent collection').selectOption('');
  await dialog.getByRole('button',{name:'Save',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('dialog'));
  assert.equal(collections.find(c=>c.id==='new').parent_id,null);
  await tree.getByRole('treeitem',{name:'Research',exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:'Move collection to trash…',exact:true}).click();
  await dialog.getByRole('button',{name:'Move to trash',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('dialog'));
  assert.ok(collections.find(c=>c.id==='child').deleted);
  await page.getByRole('button',{name:'Toggle collection trash',exact:true}).click();
  await tree.getByRole('treeitem',{name:'Research',exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:'Restore collection',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('[data-slot=context-menu-content]'));
  assert.equal(collections.find(c=>c.id==='child').deleted,false);
  assert.deepEqual(errors,[]);
  await page.screenshot({path:'/tmp/autonomics-collections.png',fullPage:true});
  console.log('Nested collections: hierarchy, recursive query, create, move, trash and restore passed');
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null) await once(server, 'exit');
}
