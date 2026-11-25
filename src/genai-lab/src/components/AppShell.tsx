import { Moon, Sparkles, Sun } from 'lucide-react'
import type { ReactNode } from 'react'
import { NavLink, useLocation } from 'react-router'
import { Button } from '@/components/ui/button'
import { TimeBar } from '@/components/TimeBar'
import { useTheme } from '@/lib/theme'
import { cn } from '@/lib/utils'

// The menu: the OTel GenAI pages read what applications report; a gateway's page reads what that gateway reports.
const MENU = [
  {
    group: 'OTel GenAI',
    items: [
      { to: '/', label: 'Metrics' },
      { to: '/breakdown', label: 'Breakdown' },
      { to: '/traces', label: 'Traces' },
    ],
  },
  { group: 'Gateways', items: [{ to: '/gateways/litellm', label: 'LiteLLM' }] },
]

// What is open belongs to its page: other pages keep only the window, and the OTel GenAI pages their filters too.
const PAGE_PARAMS = ['span', 'cell', 'at', 'rows', 'by', 'measure', 'from', 'gw_team', 'gw_key', 'gw_model']

// The menu entry a page belongs to: a trace opened from a gateway's page stays under that gateway.
function activeEntry(pathname: string, search: string): string {
  if (pathname.startsWith('/traces/') && new URLSearchParams(search).get('from') === 'litellm') return '/gateways/litellm'
  if (pathname.startsWith('/traces')) return '/traces'
  return pathname
}

function searchFor(to: string, search: string): string {
  const shared = new URLSearchParams(search)
  if (to.startsWith('/gateways/')) {
    const kept = new URLSearchParams()
    for (const key of ['after', 'before']) {
      const v = shared.get(key)
      if (v != null) kept.set(key, v)
    }
    return kept.toString() ? `?${kept}` : ''
  }
  for (const key of PAGE_PARAMS) shared.delete(key)
  return shared.toString() ? `?${shared}` : ''
}

export function AppShell({ children }: { children: ReactNode }) {
  const [theme, toggle] = useTheme()
  const { pathname, search } = useLocation()
  const active = activeEntry(pathname, search)
  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-20 border-b bg-background/90 backdrop-blur">
        <div className="mx-auto flex max-w-[1760px] flex-wrap items-center gap-x-6 gap-y-2 px-4 py-2">
          <div className="flex items-center gap-2 font-semibold">
            <Sparkles className="size-4 text-primary" />
            LLM Observability
          </div>
          <div className="ml-auto flex items-center gap-2">
            <TimeBar />
            <Button variant="ghost" size="icon-sm" onClick={toggle} aria-label="Toggle theme">
              {theme === 'dark' ? <Sun /> : <Moon />}
            </Button>
          </div>
        </div>
      </header>
      <div className="mx-auto flex max-w-[1760px] flex-col gap-4 px-4 py-4 md:flex-row">
        <nav className="flex shrink-0 flex-wrap gap-x-6 gap-y-3 md:sticky md:top-16 md:w-40 md:flex-col md:self-start">
          {MENU.map((section) => (
            <div key={section.group} className="flex flex-col gap-0.5">
              <div className="px-2.5 pb-1 text-xs font-medium tracking-wide text-muted-foreground uppercase">
                {section.group}
              </div>
              {section.items.map((t) => (
                <NavLink
                  key={t.to}
                  to={{ pathname: t.to, search: searchFor(t.to, search) }}
                  className={cn(
                    'rounded-md px-2.5 py-1 text-sm text-muted-foreground hover:text-foreground',
                    active === t.to && 'bg-muted text-foreground',
                  )}
                >
                  {t.label}
                </NavLink>
              ))}
            </div>
          ))}
        </nav>
        <main className="min-w-0 flex-1">{children}</main>
      </div>
    </div>
  )
}
