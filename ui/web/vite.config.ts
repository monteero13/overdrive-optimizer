import { readFileSync } from 'node:fs';
import { defineConfig } from 'vite';

const version = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8')).version as string;

export default defineConfig({
  // Relative asset paths: the same build works in a browser and inside the Tauri WebView.
  base: './',
  define: { __APP_VERSION__: JSON.stringify(version) },
  server: { port: 5173, strictPort: true },
  clearScreen: false,
  build: { target: 'es2022', outDir: 'dist', sourcemap: false },
});
