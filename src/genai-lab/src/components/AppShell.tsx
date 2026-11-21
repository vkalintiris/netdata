import { Moon, Sparkles, Sun } from 'lucide-react'
import type { ReactNode } from 'react'
import { NavLink, useLocation } from 'react-router'
import { Button } from '@/components/ui/button'
import { TimeBar } from '@/components/TimeBar'
import { useTheme } from '@/lib/theme'
import { cn } from '@/lib/utils'

const TABS = [
  { to: '/', label: 'Overview' },
  { to: '/calls', label: 'Calls' },
  { to: '/sessions', label: 'Sessions' },
]

export function AppShell({ children }: { children: ReactNode }) {
  const [theme, toggle] = useTheme()
  const { search } = useLocation()
  // The open call and session belong to their pages; other tabs keep only the window and filters.
  const shared = new URLSearchParams(search)
  shared.delete('call')
  shared.delete('session')
  const sharedSearch = shared.toString() ? `?${shared}` : ''
  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-20 border-b bg-background/90 backdrop-blur">
        <div className="mx-auto flex max-w-[1600px] flex-wrap items-center gap-x-6 gap-y-2 px-4 py-2">
          <div className="flex items-center gap-2 font-semibold">
            <Sparkles className="size-4 text-primary" />
            GenAI
          </div>
          <nav className="flex gap-1">
            {TABS.map((t) => (
              <NavLink
                key={t.to}
                to={{ pathname: t.to, search: sharedSearch }}
                end
                className={({ isActive }) =>
                  cn(
                    'rounded-md px-2.5 py-1 text-sm text-muted-foreground hover:text-foreground',
                    isActive && 'bg-muted text-foreground',
                  )
                }
              >
                {t.label}
              </NavLink>
            ))}
          </nav>
          <div className="ml-auto flex items-center gap-2">
            <TimeBar />
            <Button variant="ghost" size="icon-sm" onClick={toggle} aria-label="Toggle theme">
              {theme === 'dark' ? <Sun /> : <Moon />}
            </Button>
          </div>
        </div>
      </header>
      <main className="mx-auto max-w-[1600px] px-4 py-4">{children}</main>
    </div>
  )
}
