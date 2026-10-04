import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// 開発中は Rust 側（agent-calendar serve）へ API を回す。本番は同じオリジンなので不要
export default defineConfig({
  plugins: [vue()],
  server: { proxy: { '/api': 'http://127.0.0.1:8082' } },
  build: { outDir: 'dist', emptyOutDir: true },
})
