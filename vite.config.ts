import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  // Syntax minification corrupts xterm 6's local DECRQM enum binding.
  // Keep identifier/whitespace minification, but preserve protocol handler syntax.
  esbuild: { minifySyntax: false },
  root: 'web',
  server: {
    host: '127.0.0.1',
    port: 5173,
    proxy: { '/api': { target: 'http://127.0.0.1:3210', ws: true } },
  },
  build: { outDir: '../internal/webui/dist', emptyOutDir: true },
})
