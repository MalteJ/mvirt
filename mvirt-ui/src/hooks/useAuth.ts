import { useAuth as useOidcAuth } from 'react-oidc-context'

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

/// Whether the current user is a platform-wide super-admin (mvirt operator,
/// not just an Org admin). Gates the "mvirt Admin" section in the sidebar.
///
/// Per ADR-0004 we deliberately do NOT read roles from OIDC claims — each
/// provider exposes those differently ("groups", "roles", custom claim
/// paths, …) and pulling authz out of the IdP is "exactly the trap we
/// want to avoid". OIDC supplies only `(issuer, sub)`; the authoritative
/// role lives in cplane's `memberships` table at scope=Platform.
///
/// That table + the `/v1/me` endpoint that surfaces it don't exist yet
/// (pending ADR-0004 backend work). Until they do, this hook reads a
/// `mvirt-superuser=true` localStorage flag so the operator can preview
/// the section. Swap the body for `useQuery('/v1/me/memberships')` and
/// check for a Platform-scoped role once that endpoint lands.
export function useIsPlatformAdmin(): boolean {
  if (typeof window === 'undefined') return false
  return window.localStorage.getItem('mvirt-superuser') === 'true'
}
