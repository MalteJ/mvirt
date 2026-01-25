import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { KeyRound, Github } from 'lucide-react'

function GoogleIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="currentColor">
      <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4"/>
      <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/>
      <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05"/>
      <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/>
    </svg>
  )
}

function MicrosoftIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="currentColor">
      <path d="M1 1h10v10H1V1z" fill="#F25022"/>
      <path d="M13 1h10v10H13V1z" fill="#7FBA00"/>
      <path d="M1 13h10v10H1V13z" fill="#00A4EF"/>
      <path d="M13 13h10v10H13V13z" fill="#FFB900"/>
    </svg>
  )
}
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Card, CardContent } from '@/components/ui/card'
import { useAuth, OidcProvider } from '@/hooks/useAuth'

export function LoginPage() {
  const navigate = useNavigate()
  const { login } = useAuth()
  const [token, setToken] = useState('')
  const [error, setError] = useState('')
  const [isLoading, setIsLoading] = useState(false)

  const handleTokenLogin = async (e: React.FormEvent) => {
    e.preventDefault()
    setError('')
    setIsLoading(true)

    // Simulate token validation
    await new Promise((resolve) => setTimeout(resolve, 500))

    if (token.length < 8) {
      setError('Invalid token')
      setIsLoading(false)
      return
    }

    login('token', token)
    navigate('/dashboard')
  }

  const handleOidcLogin = async (provider: OidcProvider) => {
    setError('')
    setIsLoading(true)

    // Simulate OIDC flow - in real app this would redirect to provider
    await new Promise((resolve) => setTimeout(resolve, 800))

    const mockUsers: Record<OidcProvider, { name: string; email: string }> = {
      google: { name: 'Google User', email: 'user@gmail.com' },
      github: { name: 'GitHub User', email: 'user@github.com' },
      microsoft: { name: 'Microsoft User', email: 'user@outlook.com' },
    }

    login('oidc', `oidc-${provider}-mock-token`, mockUsers[provider])
    navigate('/dashboard')
  }

  return (
    <div className="min-h-screen flex flex-col bg-background">
      {/* Animated gradient background */}
      <div className="bg-gradient-animated" />

      <div className="flex-1 flex items-center justify-center p-6 relative z-10">
        <div className="w-full max-w-md space-y-8">
          {/* Logo */}
          <div className="text-center">
            <div className="inline-flex items-center justify-center">
              <div className="mr-4 flex h-14 w-14 items-center justify-center rounded-xl bg-gradient-to-br from-purple to-blue text-white text-3xl font-bold shadow-lg">
                m
              </div>
              <span className="text-3xl font-bold text-foreground">mvirt</span>
            </div>
            <p className="mt-4 text-muted-foreground">
              Sign in to manage your virtual infrastructure
            </p>
          </div>

          {/* Login options */}
          <div className="space-y-6">
            {/* OIDC Providers */}
            <Card className="border-border bg-card/50 backdrop-blur-sm">
              <CardContent className="p-6 space-y-3">
                <Button
                  variant="outline"
                  className="w-full h-11 justify-start"
                  onClick={() => handleOidcLogin('google')}
                  disabled={isLoading}
                >
                  <GoogleIcon className="mr-3 h-5 w-5" />
                  Continue with Google
                </Button>
                <Button
                  variant="outline"
                  className="w-full h-11 justify-start"
                  onClick={() => handleOidcLogin('github')}
                  disabled={isLoading}
                >
                  <Github className="mr-3 h-5 w-5" />
                  Continue with GitHub
                </Button>
                <Button
                  variant="outline"
                  className="w-full h-11 justify-start"
                  onClick={() => handleOidcLogin('microsoft')}
                  disabled={isLoading}
                >
                  <MicrosoftIcon className="mr-3 h-5 w-5" />
                  Continue with Microsoft
                </Button>
              </CardContent>
            </Card>

            {/* Divider */}
            <div className="relative">
              <div className="absolute inset-0 flex items-center">
                <div className="w-full border-t border-border" />
              </div>
              <div className="relative flex justify-center text-xs uppercase">
                <span className="bg-background px-2 text-muted-foreground">
                  Or use admin token
                </span>
              </div>
            </div>

            {/* Token login */}
            <Card className="border-border bg-card/50 backdrop-blur-sm">
              <CardContent className="p-6">
                <form onSubmit={handleTokenLogin} className="space-y-4">
                  <div className="space-y-2">
                    <Label htmlFor="token">Admin Token</Label>
                    <div className="relative">
                      <KeyRound className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                      <Input
                        id="token"
                        type="password"
                        placeholder="Enter your admin token"
                        value={token}
                        onChange={(e) => setToken(e.target.value)}
                        className="pl-10"
                        disabled={isLoading}
                      />
                    </div>
                    {error && (
                      <p className="text-sm text-destructive">{error}</p>
                    )}
                  </div>
                  <Button
                    type="submit"
                    className="w-full"
                    disabled={isLoading || !token}
                  >
                    {isLoading ? 'Signing in...' : 'Sign in with Token'}
                  </Button>
                </form>
              </CardContent>
            </Card>
          </div>

          {/* Footer */}
          <p className="text-center text-xs text-muted-foreground">
            By signing in, you agree to the{' '}
            <a href="/terms" className="text-purple-light hover:underline">
              terms of service
            </a>
          </p>
        </div>
      </div>

      {/* Pride stripe */}
      <div className="h-1 w-full flex shrink-0">
        <div className="flex-1 bg-[#e40303]" />
        <div className="flex-1 bg-[#ff8c00]" />
        <div className="flex-1 bg-[#ffed00]" />
        <div className="flex-1 bg-[#008026]" />
        <div className="flex-1 bg-[#24408e]" />
        <div className="flex-1 bg-[#732982]" />
      </div>
    </div>
  )
}
