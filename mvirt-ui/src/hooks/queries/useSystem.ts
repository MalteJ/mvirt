import { useQuery } from '@tanstack/react-query'
import { getSystemInfo } from '@/api/endpoints'

export const systemKeys = {
  all: ['system'] as const,
  info: () => [...systemKeys.all, 'info'] as const,
}

export function useSystemInfo() {
  return useQuery({
    queryKey: systemKeys.info(),
    queryFn: getSystemInfo,
    refetchInterval: 10000,
  })
}
