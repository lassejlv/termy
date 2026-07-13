import { Outlet, createRootRouteWithContext } from '@tanstack/react-router'
import type { QueryClient } from '@tanstack/react-query'
import { AuthPanel, PromptLine } from '@/panel'

export const Route = createRootRouteWithContext<{
  queryClient: QueryClient
}>()({
  component: Outlet,
  notFoundComponent: NotFound,
})

function NotFound() {
  return (
    <main className="page-shell">
      <header className="brand" aria-label="Termy Cloud">
        <span className="brand-prompt" aria-hidden="true">
          &gt;_
        </span>
        <span>Termy Cloud</span>
      </header>
      <AuthPanel title="termy cloud — 404">
        <PromptLine text="route: not found" />
        <h1>That page wandered off.</h1>
        <div className="decision-row">
          <a className="button button-primary" href="/">
            Back to Termy Cloud
          </a>
        </div>
      </AuthPanel>
      <footer>Secure browser sign-in for your Termy desktop.</footer>
    </main>
  )
}
