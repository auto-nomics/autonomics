const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');

async function send(socket, request) {
  return new Promise(resolve => {
    const stream = net.createConnection(socket);
    let response = '';
    stream.setEncoding('utf8');
    stream.setTimeout(1000, () => stream.destroy());
    stream.on('connect', () => stream.write(JSON.stringify(request) + '\n'));
    stream.on('data', chunk => {
      response += chunk;
      if (response.length > 16384) return stream.destroy();
      if (response.includes('\n')) {
        try { resolve(JSON.parse(response.slice(0, response.indexOf('\n')))); } catch { resolve(null); }
        stream.destroy();
      }
    });
    stream.on('error', () => resolve(null));
    stream.on('close', () => resolve(null));
  });
}

async function main() {
  let input = '';
  for await (const chunk of process.stdin) { input += chunk; if (input.length > 16384) throw Error('request too large'); }
  const request = JSON.parse(input);
  const directory = '/run/autonomics-vscode';
  const names = (await fs.promises.readdir(directory)).filter(name => /^bridge-\d+\.sock$/.test(name)).slice(0, 32);
  const results = (await Promise.all(names.map(name => send(path.join(directory, name), request)))).filter(result => result?.ok);
  if (!results.length) throw Error('VS Code extension host not ready');
  process.stdout.write(JSON.stringify(request.action === 'state'
    ? { ready: true, dirty: results.some(result => result.dirty === true), ...(request.debug ? { details: results.map(result => result.details) } : {}) }
    : { opened: results.some(result => result.opened === true) }));
}
main().catch(() => { process.stderr.write('VS Code extension host not ready\n'); process.exitCode = 1; });
