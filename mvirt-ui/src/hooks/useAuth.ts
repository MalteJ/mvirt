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
/// ADR-0004 defines a `platform-admin` role on the Account. The OIDC layer
/// isn't fully wired through yet — until it is, we read a localStorage
/// override (`mvirt-superuser=true`) so the operator can flip the section
/// on in their browser. When the auth model lands, swap the body for a
/// real claim check.
export function useIsPlatformAdmin(): boolean {
  if (typeof window === 'undefined') return false
  return window.localStorage.getItem('mvirt-superuser') === 'true'
}
