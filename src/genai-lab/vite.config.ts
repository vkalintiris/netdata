/// <reference types="vitest/config" />
import { readFileSync } from 'node:fs'
import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

const agent = process.env.GENAI_LAB_AGENT ?? 'http://127.0.0.1:29995'
const bearerFile = path.resolve(import.meta.dirname, '.bearer')

// A claimed Agent answers data queries only with a bearer; re-read per request so a re-mint needs no restart.
function bearer(): string | undefined {
  try {
    return readFileSync(bearerFile, 'utf8').trim() || undefined
  } catch {
    return undefined
  }
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': path.resolve(import.meta.dirname, './src') } },
  server: {
    port: Number(process.env.GENAI_LAB_PORT ?? 8094),
    proxy: {
      '/api': {
        target: agent,
        changeOrigin: true,
        configure: (proxy) => {
          proxy.on('proxyReq', (req) => {
            const token = bearer()
            if (token) req.setHeader('Authorization', `Bearer ${token}`)
          })
        },
      },
    },
  },
  test: { environment: 'node' },
})
