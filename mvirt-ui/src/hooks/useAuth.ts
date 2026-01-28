import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type AuthMethod = 'token' | 'oidc'
export type OidcProvider = 'google' | 'github' | 'microsoft'

interface AuthState {
  isAuthenticated: boolean
  authMethod: AuthMethod | null
  token: string | null
  user: {
    name: string
    email: string
    avatar?: string
  } | null
  login: (method: AuthMethod, token: string, user?: AuthState['user']) => void
  logout: () => void
}

export const useAuth = create<AuthState>()(
  persist(
    (set) => ({
      isAuthenticated: false,
      authMethod: null,
      token: null,
      user: null,
      login: (method, token, user) =>
        set({
          isAuthenticated: true,
          authMethod: method,
          token,
          user: user ?? { name: 'Admin', email: 'admin@localhost' },
        }),
      logout: () => {
        localStorage.removeItem('mvirt-auth')
        localStorage.removeItem('mvirt-project')
        set({
          isAuthenticated: false,
          authMethod: null,
          token: null,
          user: null,
        })
      },
    }),
    {
      name: 'mvirt-auth',
    }
  )
)
