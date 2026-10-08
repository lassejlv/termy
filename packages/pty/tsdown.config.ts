import { defineConfig } from 'tsdown'

export default defineConfig({
  entry: ['src/index.ts'],
  // node-pty is CommonJS; ship both so require() and import work.
  format: ['esm', 'cjs'],
  platform: 'node',
  dts: true,
  // Provides import.meta.url in the CommonJS build for the native loader.
  shims: true,
})
