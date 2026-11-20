import type { ReactNode } from 'react'
import { Skeleton } from '@/components/ui/skeleton'

export function StatTile({
  label,
  value,
  detail,
  loading,
  tone,
}: {
  label: string
  value: ReactNode
  detail?: ReactNode
  loading?: boolean
  tone?: 'error'
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-lg border bg-card px-4 py-3">
      <span className="text-xs text-muted-foreground">{label}</span>
      {loading ? (
        <Skeleton className="h-7 w-20" />
      ) : (
        <span
          className={
            'font-mono text-2xl font-semibold tabular-nums ' + (tone === 'error' ? 'text-[var(--error)]' : '')
          }
        >
          {value}
        </span>
      )}
      <span className="min-h-4 truncate text-xs text-muted-foreground">{detail}</span>
    </div>
  )
}
