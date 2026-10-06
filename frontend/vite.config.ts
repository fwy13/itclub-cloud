import { defineConfig, loadEnv } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');

  const backend = env.BACKEND_URL || 'http://127.0.0.1:8091';
  const port = Number(env.FRONTEND_PORT || '5173');

  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error('FRONTEND_PORT phải là số từ 1 đến 65535.');
  }

  return {
    plugins: [react(), tailwindcss()],

    server: {
      host: env.FRONTEND_HOST || '0.0.0.0',
      port,
      strictPort: true,

      allowedHosts: (env.FRONTEND_ALLOWED_HOSTS || '')
        .split(',')
        .map(host => host.trim())
        .filter(Boolean),

      proxy: {
        '/api': {
          target: backend,
          ws: true,
          changeOrigin: false,
        },
        '/healthz': {
          target: backend,
          changeOrigin: false,
        },
      },
    },

    build: {
      chunkSizeWarningLimit: 900,
      rollupOptions: {
        output: {
          manualChunks: {
            epub: ['epubjs'],
          },
        },
      },
    },
  };
});