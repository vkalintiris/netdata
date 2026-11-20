import { formatCount, formatSeconds } from '@/lib/format'

function shortSeconds(le: string): string {
  if (le === '+Inf') return '∞'
  const v = Number(le)
  if (v < 1) return `${Math.round(v * 1000)}ms`
  return `${Number.isInteger(v) ? v : v.toFixed(1)}s`
}

// Calls per duration bucket over the window; bars are labelled by the bucket's upper bound.
export function Distribution({ buckets }: { buckets: Record<string, number> | undefined }) {
  const entries = Object.entries(buckets ?? {})
  const total = entries.reduce((s, [, c]) => s + c, 0)
  if (!total) return <p className="py-6 text-center text-sm text-muted-foreground">No calls in this window</p>

  // Drop empty buckets at both ends so the bars that matter get the width.
  let first = entries.findIndex(([, c]) => c > 0)
  let last = entries.length - 1 - [...entries].reverse().findIndex(([, c]) => c > 0)
  first = Math.max(0, first - 1)
  last = Math.min(entries.length - 1, last + 1)
  const shown = entries.slice(first, last + 1)
  const max = Math.max(...shown.map(([, c]) => c))

  return (
    <div className="flex h-[180px] items-end gap-1 pt-2">
      {shown.map(([le, count]) => (
        <div key={le} className="flex h-full min-w-0 flex-1 flex-col items-center justify-end gap-1">
          <span className="font-mono text-[10px] text-muted-foreground tabular-nums">
            {count > 0 ? formatCount(count) : ''}
          </span>
          <div
            className="w-full rounded-t bg-[var(--chart-1)]/70"
            style={{ height: `${(count / max) * 100}%`, minHeight: count > 0 ? 2 : 0 }}
            title={`${formatCount(count)} calls ≤ ${le === '+Inf' ? '∞' : formatSeconds(Number(le))}`}
          />
          <span className="w-full truncate text-center font-mono text-[10px] text-muted-foreground">
            {shortSeconds(le)}
          </span>
        </div>
      ))}
    </div>
  )
}
