import type { ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'

// The states every list shares: failed, loading, empty, and a footer that loads the next page.
export function ListFrame({
  error,
  loading,
  empty,
  summary,
  hasMore,
  loadingMore,
  onMore,
  children,
}: {
  error: unknown
  loading: boolean
  empty: boolean
  summary: ReactNode
  hasMore: boolean
  loadingMore: boolean
  onMore: () => void
  children: ReactNode
}) {
  return (
    <div className="min-w-0 rounded-lg border bg-card">
      {error ? (
        <p className="p-6 text-sm text-destructive">{String((error as Error).message)}</p>
      ) : loading ? (
        <div className="flex flex-col gap-2 p-4">
          {Array.from({ length: 8 }, (_, i) => (
            <Skeleton key={i} className="h-6 w-full" />
          ))}
        </div>
      ) : empty ? (
        <p className="p-6 text-center text-sm text-muted-foreground">Nothing matches in this window.</p>
      ) : (
        <>
          {children}
          <div className="flex items-center justify-between gap-4 border-t px-4 py-2 text-xs text-muted-foreground">
            <span>{summary}</span>
            {hasMore && (
              <Button variant="outline" size="sm" onClick={onMore} disabled={loadingMore}>
                {loadingMore ? 'Loading…' : 'Load more'}
              </Button>
            )}
          </div>
        </>
      )}
    </div>
  )
}

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
const dayTimeFmt = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
})

// Times of a list: with the day when the window leaves today or is long.
export function listTimeFormat(range: { after: number; before: number }): Intl.DateTimeFormat {
  const long = range.before - range.after > 12 * 3600
  const otherDay = new Date(range.after * 1000).getDate() !== new Date().getDate()
  return long || otherDay ? dayTimeFmt : timeFmt
}

export const NUM = 'text-right font-mono text-xs whitespace-nowrap tabular-nums'
