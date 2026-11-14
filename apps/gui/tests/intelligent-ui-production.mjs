// Exercise the minified production bundle, including its inlined lazy module.
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:http";
import { build } from "vite";
import { chromium } from "playwright";

const gui = fileURLToPath(new URL("..", import.meta.url));
const outDir = await mkdtemp(path.join(tmpdir(), "iui-production-"));
let browser, server;
try {
  await build({ root: gui, configFile: path.join(gui, "vite.config.ts"), build: {
    outDir, emptyOutDir: true, rollupOptions: { input: path.join(gui, "tests/fixtures/intelligent-ui.html") },
  } });
  server = createServer(async (req, res) => {
    const filename = path.join(outDir, new URL(req.url, "http://localhost").pathname);
    try {
      const data = await readFile(filename);
      res.setHeader("Content-Type", filename.endsWith(".js") ? "text/javascript" : filename.endsWith(".css") ? "text/css" : "text/html");
      res.end(data);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || "/usr/bin/chromium", args: ["--no-sandbox"] });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/tests/fixtures/intelligent-ui.html`);
  await page.getByText("Plain answer", { exact: true }).waitFor();
  // Ordinary code remains inert Markdown alongside the drawing renderer.
  await page.evaluate(() => window.updateUI({text:'```javascript\nwindow.codeExecuted = true;\n```',streaming:false}));
  await page.locator('pre code').getByText('window.codeExecuted = true;', {exact:true}).waitFor();
  assert.equal(await page.evaluate(() => window.codeExecuted), undefined);
  // CopilotKit drawing channels: assert real preview BEFORE the reply is finished.
  const drawing = {
    initialHeight: 280,
    placeholderMessages: ["Drawing radius and area"],
    css: "svg{width:100%;height:160px}circle{fill:var(--color-text-info)}",
    html: ["<h3>Radius and area</h3><svg viewBox='0 0 300 160'><circle id='circle' cx='150' cy='80' r='30'/></svg>", "<label for='radius'>Radius</label><input id='radius' type='range' min='10' max='70' value='30'><output id='area'></output><canvas id='canvas' width='40' height='40'></canvas>"],
    jsFunctions: "function updateArea(){const r=Number(document.getElementById('radius').value);document.getElementById('circle').setAttribute('r',String(r));document.getElementById('area').textContent='Radius: '+r;}",
    jsExpressions: ["window.runs=(window.runs||0)+1; document.getElementById('radius').addEventListener('input',updateArea);", "updateArea();const ctx=document.getElementById('canvas').getContext('2d');ctx.fillStyle='red';ctx.fillRect(0,0,20,20);"],
  };
  const json = JSON.stringify(drawing);
  const send = async (source, streaming = true) => page.evaluate(({source,streaming}) => window.updateUI({text:'Explanation before.\n\n```generative-ui\n'+source+(streaming?'':'\n```\n\nExplanation after.'),streaming}), {source,streaming});
  const htmlStart = json.indexOf('"html":[') + 8;
  await send(json.slice(0, htmlStart + 35));
  await page.frameLocator('iframe[title="交互式绘图"]').getByText('Radius and area').waitFor();
  assert.equal(await page.locator('[aria-label="交互式解释"]').getAttribute('aria-busy'), 'true');
  const preview = page.frames().find(frame => frame !== page.mainFrame());
  await preview.evaluate(() => { window.retainedHeading = document.querySelector('h3'); });
  await send(json.slice(0, json.indexOf('"jsFunctions"') - 2));
  await page.frameLocator('iframe[title="交互式绘图"]').locator('#radius').waitFor();
  assert.equal(await preview.evaluate(() => window.retainedHeading === document.querySelector('h3')), true);
  // Closing HTML creates the execution sandbox, without waiting for the final fence.
  await send(json.slice(0, json.indexOf('"jsExpressions"')));
  await page.waitForFunction(() => document.querySelectorAll('iframe').length === 1);
  await send(json);
  const frame = page.frameLocator('iframe[title="交互式绘图"]');
  await frame.getByText('Radius: 30', {exact:true}).waitFor();
  await frame.locator('#radius').focus();
  await frame.locator('#radius').press('ArrowRight');
  await frame.getByText('Radius: 31', {exact:true}).waitFor();
  await send(json, false);
  await page.getByText('Explanation after.', {exact:true}).waitFor();
  await frame.getByText('Radius: 31', {exact:true}).waitFor();
  const execution = page.frames().find(frame => frame !== page.mainFrame());
  assert.equal(await execution.evaluate(() => window.runs), 1);
  assert.deepEqual(await execution.evaluate(() => Array.from(document.getElementById('canvas').getContext('2d').getImageData(5,5,1,1).data)), [255,0,0,255]);
  assert.equal(await execution.locator('#circle').getAttribute('r'), '31');
  assert.equal(await page.locator('iframe').getAttribute('sandbox'), 'allow-scripts ');
  assert.equal(await execution.evaluate(() => { try { return !!parent.document; } catch { return false; } }), false);
  for (const dark of [false,true]) {
    await page.evaluate(dark => document.documentElement.classList.toggle('dark',dark),dark);
    await execution.waitForFunction(expected => getComputedStyle(document.body).color === expected, dark?'rgb(201, 209, 217)':'rgb(36, 41, 47)');
    assert.equal(await execution.evaluate(() => window.runs), 1);
  }
  // Optional real CDN check, separate from the deterministic local drawing tests.
  if (process.env.INTELLIGENT_UI_LIVE_LIBRARIES === '1') {
    const libraries = {
      css: '',
      html: ["<h3>Drawing libraries</h3><output id='libraries'>Loading</output><canvas id='chart'></canvas>"],
      jsFunctions: "async function checkLibraries(){try{const [THREE,d3,chart,gsap]=await Promise.all([import('three'),import('d3'),import('chart.js/auto'),import('gsap')]);const scene=new THREE.Scene();scene.add(new THREE.Mesh(new THREE.BoxGeometry(),new THREE.MeshBasicMaterial()));const path=d3.line()([[0,0],[10,20]]);new chart.default(document.getElementById('chart'),{type:'bar',data:{labels:['A','B'],datasets:[{data:[1,2]}]},options:{animation:false,responsive:false}});document.getElementById('libraries').textContent=scene.children.length===1&&path&&gsap.gsap?'Libraries ready':'Invalid library';}catch(e){document.getElementById('libraries').textContent='Library error: '+e.message;}}",
      jsExpressions: ['checkLibraries();'],
    };
    await send(JSON.stringify(libraries),false);
    await page.frameLocator('iframe[title="交互式绘图"]').getByText('Libraries ready',{exact:true}).waitFor({timeout:60000});
    console.log('Live CDN imports: Three.js scene, D3 path, Chart.js chart and GSAP passed.');
  }
  await page.evaluate(() => window.updateUI({text:'Finished',streaming:false}));
  await page.getByText('Finished', {exact:true}).waitFor();
  assert.equal(await page.locator('iframe').count(),0);
  assert.deepEqual(errors, []);
  console.log("Production Intelligent UI: Markdown code, partial drawing preview, SVG/Canvas, ordered scripts, sandbox isolation, live themes and cleanup passed.");
} finally {
  await browser?.close();
  if (server) await new Promise(resolve => server.close(resolve));
  await rm(outDir, { recursive: true, force: true });
}
