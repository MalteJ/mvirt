import { useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { useAuth as useOidcAuth } from 'react-oidc-context'
import { useQueryClient } from '@tanstack/react-query'
import { postSignin } from '@/api/endpoints/me'
import { meKeys } from '@/hooks/queries/useMe'

export function AuthCallback() {
  const auth = useOidcAuth()
  const navigate = useNavigate()
  const qc = useQueryClient()

  useEffect(() => {
    if (auth.isLoading) return
    if (auth.error) {
      navigate(`/login?error=${encodeURIComponent(auth.error.message)}`, { replace: true })
      return
    }
    if (!auth.isAuthenticated) {
      navigate('/login', { replace: true })
      return
    }
    // OIDC redirect succeeded — kick the cplane to backfill profile claims
    // from UserInfo, then navigate. Failures here are non-fatal (display
    // name just stays as whatever the JWT body carried — usually nothing
    // for rauthy, hence the call).
    let cancelled = false
    postSignin()
      .then((me) => {
        if (!cancelled) qc.setQueryData(meKeys.all, me)
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) navigate('/dashboard', { replace: true })
      })
    return () => {
      cancelled = true
    }
  }, [auth.isAuthenticated, auth.isLoading, auth.error, navigate, qc])

  return (
    <div className="min-h-screen flex items-center justify-center bg-background text-muted-foreground">
      Authenticating…
    </div>
  )
}
