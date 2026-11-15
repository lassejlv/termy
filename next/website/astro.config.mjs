// @ts-check
import { defineConfig } from 'astro/config'
import react from '@astrojs/react'
import tailwindcss from '@tailwindcss/vite'
import { codeFrame } from './src/lib/code-frame.mjs'

export default defineConfig({
  site: 'https://next.termy.sh',
  integrations: [react()],
  vite: { plugins: [tailwindcss()] },
  markdown: {
    shikiConfig: { theme: 'tokyo-night', transformers: [codeFrame()] },
  },
})
