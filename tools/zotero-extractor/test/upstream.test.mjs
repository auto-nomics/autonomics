import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

test('vendored code, source maps and models match the pinned file manifest', async () => {
  const root = new URL('../vendor/', import.meta.url);
  const manifest = await readFile(new URL('SHA256SUMS', root), 'utf8');
  for (const line of manifest.trim().split('\n')) {
    const [expected, filename] = line.split('  ');
    const bytes = await readFile(new URL(filename, root));
    assert.equal(createHash('sha256').update(bytes).digest('hex'), expected, filename);
  }
});
