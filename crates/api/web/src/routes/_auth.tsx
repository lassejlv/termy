import { Outlet, createFileRoute } from '@tanstack/react-router'

export const Route = createFileRoute('/_auth')({
  component: AuthLayout,
})

function AuthLayout() {
  return (
    <main className="page-shell">
      <header className="brand" aria-label="Termy Cloud">
        <span className="brand-prompt" aria-hidden="true">
          &gt;_
        </span>
        <span>Termy Cloud</span>
      </header>
      <Outlet />
      <footer>Secure browser sign-in for your Termy desktop.</footer>
    </main>
  )
}
