// Images go through the build so their URLs carry a content hash: a changed
// icon or logo gets a new URL, and no browser or CDN keeps serving the old one.
import claude from '@/assets/logos/claude.svg?url'
import codex from '@/assets/logos/codex-dark.svg?url'
import docker from '@/assets/logos/docker.svg?url'
import neovim from '@/assets/logos/neovim.svg?url'
import node from '@/assets/logos/node.svg?url'
import rust from '@/assets/logos/rust-dark.svg?url'
import cellmap from '@/assets/images/cellmap.svg?url'
import icon from '@/assets/images/icon-256.png?url'
import sunFooter from '@/assets/images/sun-footer.webp?url'
import sunHero from '@/assets/images/sun-hero.webp?url'
import sunStage from '@/assets/images/sun-stage.webp?url'
import favicon from '@/assets/favicon.png?url'
import appleTouchIcon from '@/assets/apple-touch-icon.png?url'

export const logos = { claude, codex, docker, neovim, node, rust }
export type Logo = keyof typeof logos

export const images = { cellmap, icon, sunFooter, sunHero, sunStage, favicon, appleTouchIcon }
