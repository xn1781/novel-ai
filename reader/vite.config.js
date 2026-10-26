import { defineConfig } from 'vite';
import { pagesBase } from './src/pages-base.js';

export default defineConfig({
  base: pagesBase(process.env.GITHUB_REPOSITORY),
});