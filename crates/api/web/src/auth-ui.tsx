import { useMutation, useQuery } from '@tanstack/react-query'
import { startGithubSignIn } from './api'
import { authConfigQuery } from './query'

const OAUTH_RETURN_KEY = 'termy.oauth-return-to'

export function safeReturnTo(value: string | null | undefined) {
  return value?.startsWith('/') && !value.startsWith('//') ? value : '/'
}

export function consumeOAuthReturnTo() {
  const returnTo = safeReturnTo(window.sessionStorage.getItem(OAUTH_RETURN_KEY))
  window.sessionStorage.removeItem(OAUTH_RETURN_KEY)
  return returnTo
}

export function GithubSignIn({ returnTo }: { returnTo: string }) {
  const config = useQuery(authConfigQuery)
  const github = useMutation({
    mutationFn: startGithubSignIn,
    onSuccess: ({ url }) => {
      window.sessionStorage.setItem(OAUTH_RETURN_KEY, safeReturnTo(returnTo))
      window.location.assign(url)
    },
  })

  if (!config.data?.github) {
    return null
  }

  return (
    <div className="social-auth">
      <button
        className="button github-button"
        type="button"
        disabled={github.isPending}
        onClick={() => github.mutate()}
      >
        <GithubMark />
        {github.isPending ? 'Opening GitHub…' : 'Continue with GitHub'}
      </button>
      {github.error ? (
        <p className="error-message" role="alert">
          {github.error.message}
        </p>
      ) : null}
      <div className="auth-divider">
        <span>or use email</span>
      </div>
    </div>
  )
}

function GithubMark() {
  return (
    <svg className="github-mark" viewBox="0 0 24 24" aria-hidden="true">
      <path
        fill="currentColor"
        d="M12 .7a11.5 11.5 0 0 0-3.64 22.41c.58.11.79-.25.79-.56v-2.24c-3.24.7-3.92-1.38-3.92-1.38-.53-1.35-1.29-1.71-1.29-1.71-1.06-.72.08-.71.08-.71 1.17.08 1.79 1.2 1.79 1.2 1.04 1.79 2.73 1.27 3.4.97.1-.76.41-1.27.74-1.56-2.58-.29-5.3-1.29-5.3-5.69 0-1.26.45-2.29 1.2-3.09-.12-.29-.52-1.48.11-3.06 0 0 .98-.31 3.16 1.18a10.98 10.98 0 0 1 5.76 0c2.18-1.49 3.16-1.18 3.16-1.18.63 1.58.23 2.77.11 3.06.75.8 1.2 1.83 1.2 3.09 0 4.41-2.72 5.4-5.31 5.69.42.36.79 1.07.79 2.16v3.25c0 .31.21.68.8.56A11.5 11.5 0 0 0 12 .7Z"
      />
    </svg>
  )
}
