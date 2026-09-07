export default {
  root: new URL('../', import.meta.url).pathname,
  server: {
    proxy: {
      '/wasmtex': {
        target: 'https://corca-ai.github.io/wasmtex',
        changeOrigin: true,
        secure: true,
      },
      '/2025': {
        target: 'https://texlive.corca.ai/snapshots/2025-92e10d3241a312f0/2025/',
        changeOrigin: true,
        secure: true,
        rewrite: (path) => path.replace(/^\/2025/, ''),
      },
    },
  },
};
