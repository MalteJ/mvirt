import { useQuery } from '@tanstack/react-query'
import { getSystemInfo, getApiVersion } from '@/api/endpoints'

export const systemKeys = {
  all: ['system'] as const,
  info: () => [...systemKeys.all, 'info'] as const,
  health: () => [...systemKeys.all, 'health'] as const,
}

export function useSystemInfo() {
  return useQuery({
    queryKey: systemKeys.info(),
    queryFn: getSystemInfo,
    refetchInterval: 10000,
  })
}

/**
 * Polls the unauthenticated `/v1/version` endpoint to detect whether the
 * cplane is reachable. Used by the sidebar status indicator.
 *
 * `retry: false` — a failed probe shows "disconnected" immediately rather
 * than waiting through exponential backoff. The next interval tick retries.
 */
export function useApiHealth() {
  return useQuery({
    queryKey: systemKeys.health(),
    queryFn: getApiVersion,
    refetchInterval: 5000,
    refetchIntervalInBackground: true,
    retry: false,
    staleTime: 2500,
    gcTime: 10000,
  })
}
