import { useQuery } from '@tanstack/react-query'
import { queryLogs } from '@/api/endpoints'
import type { LogQueryRequest } from '@/types'

export const logKeys = {
  all: ['logs'] as const,
  queries: () => [...logKeys.all, 'query'] as const,
  query: (params: LogQueryRequest) => [...logKeys.queries(), params] as const,
}

export function useLogs(params: LogQueryRequest = {}) {
  return useQuery({
    queryKey: logKeys.query(params),
    queryFn: () => queryLogs(params),
    refetchInterval: 5000,
  })
}
