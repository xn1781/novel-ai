import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

test('mobile reader keeps scrolling inside EPUB and hides navigation chrome', () => {
  const css = readFileSync(new URL('../src/mobile-reader.css', import.meta.url), 'utf8');
  const source = readFileSync(new URL('../src/main.js', import.meta.url), 'utf8');
  assert.match(css, /body\.reader-active\s*\{[^}]*position:\s*fixed;[^}]*overflow:\s*hidden;/s);
  assert.match(css, /#book-area \.epub-container\s*\{[^}]*overscroll-behavior-y:\s*contain;[^}]*touch-action:\s*pan-y;/s);
  assert.match(css, /\.reader-footer\s*\{\s*display:\s*none;/);
  assert.match(css, /\.reader-chrome-hidden \.topbar\s*\{/);
  assert.match(source, /function setMobileChrome/);
  assert.match(source, /mobile-chrome-reveal'\)\.addEventListener\('click'/);
});
