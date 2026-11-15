// Images go through the build so their URLs carry a content hash: a changed
// icon or logo gets a new URL, and no browser or CDN keeps serving the old one.
import claude from '@/assets/logos/claude.svg?url'
import codex from '@/assets/logos/codex-dark.svg?url'
import docker from '@/assets/logos/docker.svg?url'
import neovim from '@/assets/logos/neovim.svg?url'
import node from '@/assets/logos/node.svg?url'
import rust from '@/assets/logos/rust-dark.svg?url'
import ditherDownload from '@/assets/images/dither-download.png?url'
import ditherFooter from '@/assets/images/dither-footer.png?url'
import ditherHero from '@/assets/images/dither-hero.png?url'
import ditherStage from '@/assets/images/dither-stage.png?url'
import icon from '@/assets/images/icon-256.png?url'
import favicon from '@/assets/favicon.png?url'
import appleTouchIcon from '@/assets/apple-touch-icon.png?url'

export const logos = { claude, codex, docker, neovim, node, rust }
export type Logo = keyof typeof logos

export const images = { icon, favicon, appleTouchIcon }
export const dithers = { hero: ditherHero, stage: ditherStage, download: ditherDownload, footer: ditherFooter }
