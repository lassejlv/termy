import { useMutation, useQueryClient } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import { useState, type FormEvent } from 'react'
import { register } from '@/api'
import { GithubSignIn, safeReturnTo } from '@/auth-ui'
import { AuthPanel, PromptLine } from '@/panel'
import { currentUserQuery } from '@/query'

type RegisterSearch = {
  return_to: string
}

export const Route = createFileRoute('/_auth/register')({
  validateSearch: (search: Record<string, unknown>): RegisterSearch => ({
    return_to: safeReturnTo(typeof search.return_to === 'string' ? search.return_to : '/'),
  }),
  component: RegisterPage,
})

function RegisterPage() {
  const { return_to: returnTo } = Route.useSearch()
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [validationError, setValidationError] = useState<string | null>(null)
  const signup = useMutation({
    mutationFn: () => register(name.trim(), email.trim(), password),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: currentUserQuery.queryKey })
      window.location.replace(returnTo)
    },
  })

  function submit(event: FormEvent) {
    event.preventDefault()
    if (password !== confirmPassword) {
      setValidationError('Passwords do not match')
      return
    }
    setValidationError(null)
    signup.mutate()
  }

  return (
    <AuthPanel title="termy cloud — new account">
      <PromptLine text="account: new" />
      <h1>Create your Termy account.</h1>
      <p className="lede">One account for cloud sessions across your machines.</p>
      <GithubSignIn returnTo={returnTo} />
      <form className="auth-form" onSubmit={submit}>
        <label>
          Name
          <input
            type="text"
            autoComplete="name"
            value={name}
            onChange={(event) => setName(event.target.value)}
            required
            autoFocus
          />
        </label>
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
            autoComplete="new-password"
            minLength={8}
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
          />
        </label>
        <label>
          Confirm password
          <input
            type="password"
            autoComplete="new-password"
            minLength={8}
            value={confirmPassword}
            onChange={(event) => setConfirmPassword(event.target.value)}
            required
          />
        </label>
        <button className="button button-primary" type="submit" disabled={signup.isPending}>
          {signup.isPending ? 'Creating account…' : 'Create account'}
        </button>
        {validationError ? (
          <p className="error-message" role="alert">
            {validationError}
          </p>
        ) : null}
        {signup.error ? (
          <p className="error-message" role="alert">
            {signup.error.message}
          </p>
        ) : null}
      </form>
      <p className="auth-switch">
        Already have an account? <a href={returnTo}>Login</a>
      </p>
    </AuthPanel>
  )
}
