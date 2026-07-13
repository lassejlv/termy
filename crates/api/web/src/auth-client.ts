import { createAuthClient } from 'better-auth/react'

// better-auth-rs 0.10 targets the better-auth 1.4.19 wire contract.
export const authClient = createAuthClient({
  basePath: '/auth',
  disableDefaultFetchPlugins: true,
})
