export interface Platform {
  key: string
  target: string
  os: string
  cpu: string
  libc?: string
}

export const platforms: Platform[]
export function binaryName(key: string): string
export function platformKey(): string
export function platformForTarget(target: string): Platform
export function hostPlatform(): Platform
