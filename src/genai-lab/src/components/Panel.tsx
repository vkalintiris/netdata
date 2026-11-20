import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export function Panel({
  title,
  subtitle,
  action,
  className,
  children,
}: {
  title: string
  subtitle?: string
  action?: ReactNode
  className?: string
  children: ReactNode
}) {
  return (
    <section className={cn('flex min-w-0 flex-col rounded-lg border bg-card p-3', className)}>
      <header className="mb-2 flex items-baseline gap-2">
        <h3 className="text-sm font-medium">{title}</h3>
        {subtitle && <span className="truncate text-xs text-muted-foreground">{subtitle}</span>}
        <div className="ml-auto">{action}</div>
      </header>
      {children}
    </section>
  )
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return <h2 className="mt-6 mb-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">{children}</h2>
}
