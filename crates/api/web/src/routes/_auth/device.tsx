import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useState, type FormEvent } from 'react'
import { approveDevice, denyDevice, signIn } from '@/api'
import { GithubSignIn } from '@/auth-ui'
import { AuthPanel, PromptLine } from '@/panel'
import { currentUserQuery } from '@/query'

type DeviceSearch = {
  user_code: string
}

export const Route = createFileRoute('/_auth/device')({
  validateSearch: (search: Record<string, unknown>): DeviceSearch => ({
    user_code: normalizeCode(typeof search.user_code === 'string' ? search.user_code : ''),
  }),
  component: DeviceAuthorization,
})

function normalizeCode(value: string) {
  return value.trim().replaceAll('-', '').toUpperCase()
}

function displayCode(value: string) {
  return value.length === 8 ? `${value.slice(0, 4)}-${value.slice(4)}` : value
}

function DeviceAuthorization() {
  const { user_code: userCode } = Route.useSearch()
  const navigate = useNavigate({ from: '/device' })
  const queryClient = useQueryClient()
  const user = useQuery(currentUserQuery)
  const [typedCode, setTypedCode] = useState(userCode)
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [decision, setDecision] = useState<'approved' | 'denied' | null>(null)

  const login = useMutation({
    mutationFn: () => signIn(email.trim(), password),
    onSuccess: async () => {
      setPassword('')
      await queryClient.invalidateQueries({ queryKey: currentUserQuery.queryKey })
    },
  })
  const approve = useMutation({
    mutationFn: () => approveDevice(userCode),
    onSuccess: () => setDecision('approved'),
  })
  const deny = useMutation({
    mutationFn: () => denyDevice(userCode),
    onSuccess: () => setDecision('denied'),
  })

  if (!userCode) {
    function continueWithCode(event: FormEvent) {
      event.preventDefault()
      const normalized = normalizeCode(typedCode)
      if (normalized) {
        void navigate({ search: { user_code: normalized } })
      }
    }

    return (
      <AuthPanel title="termy cloud — connect device">
        <PromptLine text="device: connect" />
        <h1>Enter the code from Termy.</h1>
        <form className="auth-form" onSubmit={continueWithCode}>
          <label>
            Device code
            <input
              className="code-input"
              inputMode="text"
              autoCapitalize="characters"
              autoComplete="one-time-code"
              maxLength={9}
              value={typedCode}
              onChange={(event) => setTypedCode(event.target.value)}
              placeholder="ABCD-EFGH"
              required
              autoFocus
            />
          </label>
          <button className="button button-primary" type="submit">
            Continue
          </button>
        </form>
      </AuthPanel>
    )
  }

  if (decision) {
    return (
      <AuthPanel title="termy cloud — connect device" live>
        <PromptLine text={`device: ${decision}`} />
        <h1>{decision === 'approved' ? 'Termy is connected.' : 'Request denied.'}</h1>
        <p className="lede">
          {decision === 'approved'
            ? 'You can close this tab and head back to the desktop app.'
            : 'No session was shared with the desktop app.'}
        </p>
      </AuthPanel>
    )
  }

  if (user.isPending) {
    return (
      <AuthPanel title="termy cloud — connect device" live>
        <PromptLine text="device: checking session" />
        <h1>One second.</h1>
      </AuthPanel>
    )
  }

  function submitLogin(event: FormEvent) {
    event.preventDefault()
    login.mutate()
  }

  if (!user.data) {
    const returnTo = `/device?user_code=${encodeURIComponent(userCode)}`
    return (
      <AuthPanel title="termy cloud — connect device">
        <PromptLine text={`device code: ${displayCode(userCode)}`} />
        <h1>Login to approve Termy.</h1>
        <p className="lede">Your password stays in this browser. The app receives a session only after you approve it.</p>
        <GithubSignIn returnTo={returnTo} />
        <form className="auth-form" onSubmit={submitLogin}>
          <label>
            Email
            <input
              type="email"
              autoComplete="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              required
              autoFocus
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
          Need an account? <a href={`/register?return_to=${encodeURIComponent(returnTo)}`}>Register</a>
        </p>
      </AuthPanel>
    )
  }

  const busy = approve.isPending || deny.isPending
  return (
    <AuthPanel title="termy cloud — connect device">
      <PromptLine text={`device code: ${displayCode(userCode)}`} />
      <h1>Connect this Termy?</h1>
      <p className="lede">
        Signed in as <strong>{user.data.email}</strong>. Only approve if you started this login from your desktop app.
      </p>
      <div className="decision-row">
        <button
          className="button button-primary"
          type="button"
          disabled={busy}
          onClick={() => approve.mutate()}
        >
          {approve.isPending ? 'Connecting…' : 'Approve Termy'}
        </button>
        <button
          className="button button-secondary"
          type="button"
          disabled={busy}
          onClick={() => deny.mutate()}
        >
          Deny
        </button>
      </div>
      {approve.error ? <ErrorMessage error={approve.error} /> : null}
      {deny.error ? <ErrorMessage error={deny.error} /> : null}
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
