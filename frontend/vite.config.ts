import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      '/api': { target: 'http://127.0.0.1:8091', ws: true, changeOrigin: false },
      '/healthz': 'http://127.0.0.1:8091',
    },
  },
  build: { chunkSizeWarningLimit: 900, rollupOptions: { output: { manualChunks: { epub: ['epubjs'] } } } },
});
