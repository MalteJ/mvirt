import { ReactNode } from 'react'
import { Sidebar } from './Sidebar'
import { Header } from './Header'

interface LayoutProps {
  children: ReactNode
}

export function Layout({ children }: LayoutProps) {
  return (
    <div className="flex h-screen flex-col bg-background">
      {/* Animated gradient background */}
      <div className="bg-gradient-animated" />

      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <div className="relative z-10 flex flex-1 flex-col overflow-hidden">
          <Header />
          <main className="main-content flex-1 overflow-auto p-6">
            {children}
          </main>
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
