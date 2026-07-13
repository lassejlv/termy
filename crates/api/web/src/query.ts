import { queryOptions } from '@tanstack/react-query'
import { getAuthConfig, getCurrentUser, getProjects, getRailwayStatus } from './api'

export const currentUserQuery = queryOptions({
  queryKey: ['current-user'],
  queryFn: getCurrentUser,
  staleTime: 30_000,
  retry: false,
})

export const authConfigQuery = queryOptions({
  queryKey: ['auth-config'],
  queryFn: getAuthConfig,
  staleTime: Number.POSITIVE_INFINITY,
  retry: false,
})

export const railwayStatusQuery = queryOptions({
  queryKey: ['railway-status'],
  queryFn: getRailwayStatus,
  staleTime: 30_000,
  retry: false,
})

export const projectsQuery = queryOptions({
  queryKey: ['projects'],
  queryFn: getProjects,
  retry: false,
})
