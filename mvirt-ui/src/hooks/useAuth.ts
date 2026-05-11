import { useAuth as useOidcAuth } from 'react-oidc-context'
import { useMe } from './queries/useMe'

export interface AuthUser {
  name: string
  email: string
  avatar?: string
}

/**
 * Thin wrapper around react-oidc-context's useAuth. Keeps the API surface
 * (isAuthenticated, user, logout) compatible with the rest of the app while
 * delegating to the OIDC client for the actual auth state.
 */
export function useAuth() {
  const auth = useOidcAuth()
  const profile = auth.user?.profile

  const user: AuthUser | null = profile
    ? {
        name:
          (profile.name as string | undefined) ??
          (profile.preferred_username as string | undefined) ??
          (profile.email as string | undefined) ??
          'User',
        email: (profile.email as string | undefined) ?? '',
        avatar: profile.picture as string | undefined,
      }
    : null

  return {
    isAuthenticated: auth.isAuthenticated,
    isLoading: auth.isLoading,
    error: auth.error,
    user,
    accessToken: auth.user?.access_token ?? null,
    signIn: () => auth.signinRedirect(),
    logout: () => auth.signoutRedirect(),
  }
}

/// Whether the current user is a platform-wide super-admin (ADR-0004
/// scope=Platform, role=platform-admin). Authoritative source is the
/// cplane's memberships table, surfaced via `/v1/me`. In dev mode (no
/// JWT validator on the cplane) /v1/me returns 401 → the hook falls back
/// to the `mvirt-superuser` localStorage flag so the operator can still
/// preview the section. Production deployments rely solely on the API.
///
/// Deliberately NOT reading OIDC claims — providers expose roles
/// differently and pulling authz out of the IdP is the trap ADR-0004
/// avoids. OIDC supplies only `(issuer, sub)`; cplane owns the role.
export function useIsPlatformAdmin(): boolean {
  const { data } = useMe()
  if (data && data.isPlatformAdmin) return true
  // data === null → cplane returned 401 (auth disabled in dev). Fall
  // through to the localStorage override.
  if (data === null && typeof window !== 'undefined') {
    return window.localStorage.getItem('mvirt-superuser') === 'true'
  }
  return false
}
