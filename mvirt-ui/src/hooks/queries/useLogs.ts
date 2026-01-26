import { useQuery } from '@tanstack/react-query'
import { queryLogs } from '@/api/endpoints'
import { useProject } from '@/hooks/useProject'
import type { LogQueryRequest } from '@/types'

export const logKeys = {
  all: ['logs'] as const,
  queries: () => [...logKeys.all, 'query'] as const,
  query: (params: LogQueryRequest) => [...logKeys.queries(), params] as const,
}

export function useLogs(params: Omit<LogQueryRequest, 'projectId'> = {}) {
  const { currentProject } = useProject()
  const fullParams: LogQueryRequest = {
    ...params,
    projectId: currentProject?.id,
  }
  return useQuery({
    queryKey: logKeys.query(fullParams),
    queryFn: () => queryLogs(fullParams),
    enabled: !!currentProject,
    refetchInterval: 5000,
  })
}
