import { Link } from 'react-router-dom'
import { ArrowLeft } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'

export function TermsPage() {
  return (
    <div className="min-h-screen flex flex-col bg-background">
      <div className="bg-gradient-animated" />

      <div className="flex-1 p-6 relative z-10">
        <div className="max-w-2xl mx-auto space-y-6">
          <div>
            <Button variant="ghost" size="sm" asChild>
              <Link to="/login">
                <ArrowLeft className="mr-2 h-4 w-4" />
                Back to login
              </Link>
            </Button>
          </div>

          <Card className="border-border bg-card/50 backdrop-blur-sm">
            <CardContent className="p-8 prose prose-invert prose-sm max-w-none">
              <h1 className="text-2xl font-bold text-foreground">Terms of Service</h1>

              <p className="text-muted-foreground">Last updated: January 2025</p>

              <h2 className="text-lg font-semibold text-foreground mt-6">1. Acceptance of Terms</h2>
              <p className="text-muted-foreground">
                By accessing and using mvirt, you accept and agree to be bound by the terms
                and provision of this agreement.
              </p>

              <h2 className="text-lg font-semibold text-foreground mt-6">2. Use License</h2>
              <p className="text-muted-foreground">
                Permission is granted to use mvirt for managing virtual infrastructure
                within your organization. This license shall automatically terminate if you
                violate any of these restrictions.
              </p>

              <h2 className="text-lg font-semibold text-foreground mt-6">3. Disclaimer</h2>
              <p className="text-muted-foreground">
                The materials on mvirt are provided on an 'as is' basis. mvirt makes no
                warranties, expressed or implied, and hereby disclaims and negates all other
                warranties including, without limitation, implied warranties or conditions of
                merchantability, fitness for a particular purpose, or non-infringement of
                intellectual property or other violation of rights.
              </p>

              <h2 className="text-lg font-semibold text-foreground mt-6">4. Limitations</h2>
              <p className="text-muted-foreground">
                In no event shall mvirt or its suppliers be liable for any damages (including,
                without limitation, damages for loss of data or profit, or due to business
                interruption) arising out of the use or inability to use mvirt.
              </p>

              <h2 className="text-lg font-semibold text-foreground mt-6">5. Privacy</h2>
              <p className="text-muted-foreground">
                Your use of mvirt is also governed by our Privacy Policy. Please review our
                Privacy Policy, which also governs the site and informs users of our data
                collection practices.
              </p>

              <h2 className="text-lg font-semibold text-foreground mt-6">6. Governing Law</h2>
              <p className="text-muted-foreground">
                Any claim relating to mvirt shall be governed by the laws of the jurisdiction
                in which the service is operated without regard to its conflict of law provisions.
              </p>
            </CardContent>
          </Card>
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
