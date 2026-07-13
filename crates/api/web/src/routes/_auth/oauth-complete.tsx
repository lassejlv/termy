import { useMutation } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useRef } from 'react'
import { completeGithubSignIn } from '@/api'
import { consumeOAuthReturnTo } from '@/auth-ui'
import { AuthPanel, PromptLine } from '@/panel'

type OAuthCompleteSearch = {
  code: string
  state: string
  error: string
}

export const Route = createFileRoute('/_auth/oauth-complete')({
  validateSearch: (search: Record<string, unknown>): OAuthCompleteSearch => ({
    code: typeof search.code === 'string' ? search.code : '',
    state: typeof search.state === 'string' ? search.state : '',
    error: typeof search.error === 'string' ? search.error : '',
  }),
  component: OAuthComplete,
})

function OAuthComplete() {
  const { code, state, error: providerError } = Route.useSearch()
  const callbackStarted = useRef(false)
  const callback = useMutation({
    mutationFn: () => completeGithubSignIn(code, state),
    onSuccess: () => window.location.replace(consumeOAuthReturnTo()),
  })

  useEffect(() => {
    if (!providerError && code && state && !callbackStarted.current) {
      callbackStarted.current = true
      callback.mutate()
    }
  }, [callback, code, providerError, state])

  if (providerError || !code || !state || callback.isError) {
    return (
      <AuthPanel title="termy cloud — github">
        <PromptLine text="github: failed" />
        <h1>GitHub login did not finish.</h1>
        <p className="lede">Return to login and try again.</p>
        <div className="decision-row">
          <a className="button button-primary" href="/">
            Back to login
          </a>
        </div>
      </AuthPanel>
    )
  }

  return (
    <AuthPanel title="termy cloud — github" live>
      <PromptLine text="github: connected" />
      <h1>Taking you back to Termy.</h1>
    </AuthPanel>
  )
}
