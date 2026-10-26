import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

test('all static reader DOM id references exist in the HTML', () => {
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
  const source = readFileSync(new URL('../src/main.js', import.meta.url), 'utf8');
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]));
  const references = [...source.matchAll(/\$\('([^']+)'\)/g)].map(match => match[1]);
  assert.match(html, /<script\s+type="module"\s+src="\/src\/main\.js"><\/script>/, 'Vite entry point must be present in reader/index.html');
  assert.equal([...html.matchAll(/<script\s+type="module"\s+src="\/src\/main\.js"><\/script>/g)].length, 1, 'Vite entry point should appear exactly once');
  for (const id of references) assert.ok(ids.has(id), `Missing id="${id}" in reader/index.html`);
  for (const view of ['unlock', 'library', 'reader']) assert.ok(ids.has(`${view}-view`));
  for (const panel of ['chapters', 'settings']) assert.ok(ids.has(`${panel}-panel`));
});