import { useEffect, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { LogIn } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { useAuth } from '@/hooks/useAuth'

export function LoginPage() {
  const { signIn, error: authError } = useAuth()
  const [searchParams] = useSearchParams()
  const [signingIn, setSigningIn] = useState(false)

  const error = authError?.message ?? searchParams.get('error') ?? null

  useEffect(() => {
    document.title = 'Sign in · mvirt'
  }, [])

  const handleSignIn = () => {
    setSigningIn(true)
    signIn().catch(() => setSigningIn(false))
  }

  return (
    <div className="min-h-screen flex flex-col bg-background">
      <div className="bg-gradient-animated" />

      <div className="flex-1 flex items-center justify-center p-6 relative z-10">
        <div className="w-full max-w-md space-y-8">
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

          <Card className="border-border bg-card/50 backdrop-blur-sm">
            <CardContent className="p-6 space-y-4">
              <Button
                className="w-full h-11"
                onClick={handleSignIn}
                disabled={signingIn}
              >
                <LogIn className="mr-2 h-4 w-4" />
                {signingIn ? 'Redirecting…' : 'Sign in with mvirt SSO'}
              </Button>
              {error && (
                <p className="text-sm text-destructive text-center">{error}</p>
              )}
            </CardContent>
          </Card>

          <p className="text-center text-xs text-muted-foreground">
            By signing in, you agree to the{' '}
            <a href="/terms" className="text-purple-light hover:underline">
              terms of service
            </a>
          </p>
        </div>
      </div>

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
