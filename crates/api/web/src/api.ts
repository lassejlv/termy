import { authClient } from './auth-client'

export type CloudUser = {
  id: string
  email: string
  name: string | null
}

export type AuthConfig = {
  github: boolean
}

type ErrorBody = {
  message?: string
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

type AuthClientResult<T> = {
  data: T | null
  error: { message?: string; status: number } | null
}

function unwrapAuthResult<T>({ data, error }: AuthClientResult<T>): T {
  if (error) {
    throw new ApiError(error.message || 'Authentication failed', error.status)
  }
  if (data === null) {
    throw new ApiError('The auth server returned an empty response', 500)
  }
  return data
}

async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    credentials: 'include',
    headers: {
      Accept: 'application/json',
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...init?.headers,
    },
  })

  const body = (await response.json().catch(() => null)) as (T & ErrorBody) | null
  if (!response.ok) {
    throw new ApiError(body?.message || `Request failed (${response.status})`, response.status)
  }
  if (body === null) {
    if (response.status === 204) {
      return undefined as T
    }
    throw new ApiError('The server returned an empty response', response.status)
  }
  return body
}

export async function getCurrentUser(): Promise<CloudUser | null> {
  try {
    return await apiRequest<CloudUser>('/api/me')
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
      return null
    }
    throw error
  }
}

export function getAuthConfig() {
  return apiRequest<AuthConfig>('/api/auth-config')
}

export async function signIn(email: string, password: string) {
  return unwrapAuthResult(await authClient.signIn.email({ email, password, rememberMe: true }))
}

export async function register(name: string, email: string, password: string) {
  return unwrapAuthResult(await authClient.signUp.email({ name, email, password }))
}

export async function startGithubSignIn() {
  const result = unwrapAuthResult(
    await authClient.signIn.social({
      provider: 'github',
      callbackURL: `${window.location.origin}/oauth-complete`,
    }),
  )
  if (!result.url) {
    throw new ApiError('GitHub did not return a login URL', 500)
  }
  return { url: result.url }
}

export function completeGithubSignIn(code: string, state: string) {
  const query = new URLSearchParams({ code, state })
  return apiRequest(`/auth/callback/github?${query}`)
}

export async function signOut() {
  return unwrapAuthResult(await authClient.signOut())
}

export type RailwayStatus = {
  connected: boolean
  account_name?: string | null
  expires_at?: string | null
}

export type SessionSummary = {
  id: string
  status: string
}

export type Project = {
  id: string
  name: string
  repo_url: string
  default_branch: string
  setup_command: string | null
  active_session: SessionSummary | null
}

export type SessionStatus = {
  id: string
  status: string
  status_detail: string | null
}

export type ConnectionInfo = {
  ssh_host: string
  ssh_user: string
  ssh_command: string
}

export type CreateProjectInput = {
  name: string
  repo_url: string
  default_branch: string
  setup_command: string | null
}

export function getRailwayStatus() {
  return apiRequest<RailwayStatus>('/api/providers/railway')
}

export function disconnectRailway() {
  return apiRequest<unknown>('/api/providers/railway', { method: 'DELETE' })
}

export function getProjects() {
  return apiRequest<{ projects: Project[] }>('/api/projects').then((body) => body.projects)
}

export function createProject(input: CreateProjectInput) {
  return apiRequest<Project>('/api/projects', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function deleteProject(projectId: string) {
  return apiRequest<unknown>(`/api/projects/${projectId}`, { method: 'DELETE' })
}

export function startSession(projectId: string) {
  return apiRequest<{ session_id: string; status: string }>(
    `/api/projects/${projectId}/sessions`,
    { method: 'POST', body: JSON.stringify({}) },
  )
}

export function getSession(sessionId: string) {
  return apiRequest<SessionStatus>(`/api/sessions/${sessionId}`)
}

export function getSessionConnection(sessionId: string) {
  return apiRequest<ConnectionInfo>(`/api/sessions/${sessionId}/connection`)
}

export function stopSession(sessionId: string) {
  return apiRequest<unknown>(`/api/sessions/${sessionId}`, { method: 'DELETE' })
}

export function approveDevice(userCode: string) {
  return apiRequest<{ status: boolean }>('/auth/device/approve', {
    method: 'POST',
    body: JSON.stringify({ userCode }),
  })
}

export function denyDevice(userCode: string) {
  return apiRequest<{ status: boolean }>('/auth/device/deny', {
    method: 'POST',
    body: JSON.stringify({ userCode }),
  })
}
