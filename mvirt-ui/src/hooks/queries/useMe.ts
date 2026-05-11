import { useQuery } from '@tanstack/react-query'
import { getMe } from '@/api/endpoints'

export const meKeys = {
  all: ['me'] as const,
}

/** Current user — Account + memberships. Returns `undefined` until the
 *  request resolves; the underlying query swallows 401 (server returns
 *  it when auth is disabled in dev) and yields `null` so callers can
 *  fall through to a dev-mode override. */
export function useMe() {
  return useQuery({
    queryKey: meKeys.all,
    queryFn: async () => {
      try {
        return await getMe()
      } catch (err) {
        // 401 is expected when auth is off (dev mode without a token).
        // Surface as null instead of an error so the localStorage stub
        // can take over for the platform-admin gate.
        const status = (err as { code?: number }).code
        if (status === 401) return null
        throw err
      }
    },
    staleTime: 60_000,
  })
}
