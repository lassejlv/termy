import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, createFileRoute } from '@tanstack/react-router'
import { useState, type FormEvent } from 'react'
import { signIn, signOut } from '@/api'
import { GithubSignIn } from '@/auth-ui'
import { AuthPanel, PromptLine } from '@/panel'
import { currentUserQuery } from '@/query'

export const Route = createFileRoute('/_auth/')({
  component: CloudHome,
})

function CloudHome() {
  const queryClient = useQueryClient()
  const user = useQuery(currentUserQuery)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const login = useMutation({
    mutationFn: () => signIn(email.trim(), password),
    onSuccess: async () => {
      setPassword('')
      await queryClient.invalidateQueries({ queryKey: currentUserQuery.queryKey })
    },
  })
  const logout = useMutation({
    mutationFn: signOut,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: currentUserQuery.queryKey })
    },
  })

  if (user.isPending) {
    return <LoadingPanel />
  }

  if (user.data) {
    return (
      <AuthPanel title="termy cloud — session">
        <PromptLine text="session: active" />
        <h1>You’re signed in.</h1>
        <p className="lede">This browser is connected as {user.data.email}.</p>
        <div className="decision-row">
          <Link className="button button-primary" to="/dashboard">
            Open dashboard
          </Link>
          <button
            className="button button-secondary"
            type="button"
            disabled={logout.isPending}
            onClick={() => logout.mutate()}
          >
            {logout.isPending ? 'Signing out…' : 'Sign out'}
          </button>
        </div>
        {logout.error ? <ErrorMessage error={logout.error} /> : null}
      </AuthPanel>
    )
  }

  function submit(event: FormEvent) {
    event.preventDefault()
    login.mutate()
  }

  return (
    <AuthPanel title="termy cloud — sign in">
      <PromptLine text="cloud: waiting for you" />
      <h1>Sign in to Termy.</h1>
      <p className="lede">Use the same account you use for Termy Cloud.</p>
      <GithubSignIn returnTo="/" />
      <form className="auth-form" onSubmit={submit}>
        <label>
          Email
          <input
            type="email"
            autoComplete="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            required
          />
        </label>
        <label>
          Password
          <input
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
          />
        </label>
        <button className="button button-primary" type="submit" disabled={login.isPending}>
          {login.isPending ? 'Signing in…' : 'Login'}
        </button>
        {login.error ? <ErrorMessage error={login.error} /> : null}
      </form>
      <p className="auth-switch">
        New to Termy? <a href="/register">Create an account</a>
      </p>
    </AuthPanel>
  )
}

function LoadingPanel() {
  return (
    <AuthPanel title="termy cloud" live>
      <PromptLine text="session: checking" />
      <h1>One second.</h1>
    </AuthPanel>
  )
}

function ErrorMessage({ error }: { error: Error }) {
  return (
    <p className="error-message" role="alert">
      {error.message}
    </p>
  )
}
