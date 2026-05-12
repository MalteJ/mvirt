import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router-dom'
import { AuthProvider } from 'react-oidc-context'
import App from './App'
import { userManager } from './auth/userManager'
import './index.css'

// Initialize theme from localStorage or default to dark
const storedTheme = localStorage.getItem('mvirt-theme')
const theme = storedTheme ? JSON.parse(storedTheme).state?.theme : 'dark'
document.documentElement.classList.add(theme)

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5000,
      refetchOnWindowFocus: false,
    },
  },
})

// Strip OIDC code/state params from URL after successful sign-in callback so a
// reload doesn't replay the (now invalid) authorization code.
const onSigninCallback = () => {
  window.history.replaceState({}, document.title, '/')
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AuthProvider userManager={userManager} onSigninCallback={onSigninCallback}>
      <QueryClientProvider client={queryClient}>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </QueryClientProvider>
    </AuthProvider>
  </StrictMode>,
)
