const vscode = require('vscode');
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');

function activate(context) {
  const directory = process.env.AUTONOMICS_VSCODE_BRIDGE_DIR;
  if (!directory) return;
  const socket = path.join(directory, `bridge-${process.pid}.sock`);
  const server = net.createServer(connection => {
    let input = '';
    connection.setEncoding('utf8');
    connection.setTimeout(3000, () => connection.destroy());
    connection.on('error', () => {});
    connection.on('data', chunk => {
      input += chunk;
      if (input.length > 16384) return connection.destroy();
      if (!input.includes('\n')) return;
      connection.pause();
      void handle(input.slice(0, input.indexOf('\n'))).then(
        result => connection.end(JSON.stringify({ ok: true, ...result }) + '\n'),
        () => connection.end(JSON.stringify({ ok: false }) + '\n'),
      );
    });
  });
  server.on('error', error => console.error('Autonomics bridge:', error.message));
  server.listen(socket, () => fs.chmod(socket, 0o666, () => {}));
  context.subscriptions.push({ dispose() { server.close(); try { fs.unlinkSync(socket); } catch {} } });
}

async function handle(input) {
  const request = JSON.parse(input);
  if (request.action === 'state') {
    const documents = vscode.workspace.textDocuments;
    return { dirty: documents.some(document => document.isDirty), details: request.debug ? documents.map(document => ({ scheme: document.uri.scheme, dirty: document.isDirty })) : undefined };
  }
  if (request.action !== 'open' || typeof request.path !== 'string' || !request.path || path.isAbsolute(request.path)) throw Error('invalid request');
  const resolved = await fs.promises.realpath(path.join('/work', request.path));
  const root = await fs.promises.realpath('/work');
  if (!resolved.startsWith(root + path.sep) || !(await fs.promises.stat(resolved)).isFile()) throw Error('outside workspace');
  const line = Number.isSafeInteger(request.line) && request.line > 0 ? request.line - 1 : 0;
  const column = Number.isSafeInteger(request.column) && request.column > 0 ? request.column - 1 : 0;
  const selection = new vscode.Range(line, column, line, column);
  await vscode.window.showTextDocument(vscode.Uri.file(resolved), { preview: false, selection });
  return { opened: true };
}

exports.activate = activate;
