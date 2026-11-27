import { defineConfig, defaultClientConditions } from 'vite';
import react from '@vitejs/plugin-react';

const backendUrl = process.env.APP_BASE_URL || `http://localhost:${process.env.PORT || '8081'}`;

export default defineConfig({
  plugins: [react()],
  resolve: {
    // ONNX Runtime (under transformers.js and parakeet.js) loads its 20+ MB
    // WebAssembly from the CDN when a voice model is first used, instead of
    // copying it into dist/ (and so into git and the desktop installer).
    conditions: ['onnxruntime-web-use-extern-wasm', ...defaultClientConditions],
  },
  server: {
    port: 3000,
    proxy: {
      '/api': backendUrl,
      '/threads': backendUrl,
      '/tasks': backendUrl,
      '/approvals': backendUrl,
      '/agents': backendUrl,
      '/procedures': backendUrl,
      '/device': backendUrl,
      '/auth': backendUrl,
      '/signin': backendUrl,
    },
  },
  // The Kokoro voice runs in a worker that loads its engine on demand.
  worker: { format: 'es' },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // The voice engines (transformers.js, Kokoro) are large but only load
    // on demand through dynamic import(), never with the app itself.
    chunkSizeWarningLimit: 1500,
    rolldownOptions: {
      output: {
        // Screens load on demand; the icons they share go in one file
        // instead of one tiny file each, which would be dozens of requests.
        codeSplitting: {
          groups: [{ name: 'icons', test: /[\\/]node_modules[\\/]lucide-react[\\/]/ }],
        },
      },
    },
  },
});
