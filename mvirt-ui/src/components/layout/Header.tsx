import { Bell, Moon, Sun } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useTheme } from '@/hooks/useTheme'

export function Header() {
  const { theme, toggleTheme } = useTheme()

  return (
    <header className="flex h-14 items-center justify-end border-b border-border bg-card/80 backdrop-blur-xl px-6">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="icon" className="hover:bg-purple/20 hover:text-purple-light">
          <Bell className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="hover:bg-purple/20 hover:text-purple-light"
          onClick={toggleTheme}
          title={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
        >
          {theme === 'dark' ? (
            <Sun className="h-4 w-4" />
          ) : (
            <Moon className="h-4 w-4" />
          )}
        </Button>
      </div>
    </header>
  )
}
