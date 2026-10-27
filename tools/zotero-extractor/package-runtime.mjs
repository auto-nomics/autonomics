// Build a self-contained runtime directory for a native release. Run on the
// target OS/architecture so the canvas binding matches the packaged server.
import { mkdir, cp, copyFile, chmod, access } from 'node:fs/promises';
import { constants } from 'node:fs';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { once } from 'node:events';

const root = path.dirname(fileURLToPath(import.meta.url));
const destination = process.argv[2] && path.resolve(process.argv[2]);
if (!destination) throw new Error('Usage: node package-runtime.mjs <new-output-directory>');
if (!['linux', 'darwin'].includes(process.platform)) throw new Error('Release runtime packaging currently supports Linux and macOS');
try {
  await access(destination);
  throw new Error(`Output directory already exists: ${destination}`);
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
}
await mkdir(destination, { recursive: true });
for (const name of ['extract.mjs', 'package.json', 'package-lock.json', 'UPSTREAM.json', 'THIRD_PARTY.md', 'README.md', 'vendor']) {
  await cp(path.join(root, name), path.join(destination, name), { recursive: true });
}
const npm = spawn('npm', ['ci', '--omit=dev', '--no-audit', '--no-fund'], { cwd: destination, stdio: 'inherit' });
const [code] = await once(npm, 'exit');
if (code !== 0) throw new Error(`Installing runtime dependencies failed (${code})`);
await copyFile(process.execPath, path.join(destination, 'node'));
await chmod(path.join(destination, 'node'), 0o755);
const candidates = [
  process.env.ZOTERO_NODE_LICENSE,
  path.resolve(path.dirname(process.execPath), '../LICENSE'),
  '/usr/share/licenses/nodejs/LICENSE',
].filter(Boolean);
let license;
for (const candidate of candidates) {
  try { await access(candidate, constants.R_OK); license = candidate; break; } catch { /* Try the next installation layout. */ }
}
if (!license) throw new Error('Node LICENSE was not found; set ZOTERO_NODE_LICENSE before packaging');
await copyFile(license, path.join(destination, 'NODE-LICENSE'));
console.log(`Packaged Zotero extractor, production dependencies and Node ${process.version} at ${destination}`);
