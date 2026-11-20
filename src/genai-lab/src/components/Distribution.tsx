import { formatCount, formatSeconds } from '@/lib/format'

function shortSeconds(le: string): string {
  if (le === '+Inf') return '∞'
  const v = Number(le)
  if (v < 1) return `.${String(Math.round(v * 100)).padStart(2, '0')}s`
  return `${v < 10 ? Number(v.toFixed(1)) : Math.round(v)}s`
}

// Calls per duration bucket over the window; bars are labelled by the bucket's upper bound.
export function Distribution({ buckets }: { buckets: Record<string, number> | undefined }) {
  const entries = Object.entries(buckets ?? {})
  const total = entries.reduce((s, [, c]) => s + c, 0)
  if (!total) return <p className="py-6 text-center text-sm text-muted-foreground">No calls in this window</p>

  // Drop empty buckets at both ends so the bars that matter get the width.
  const first = entries.findIndex(([, c]) => c > 0)
  const last = entries.length - 1 - [...entries].reverse().findIndex(([, c]) => c > 0)
  const shown = entries.slice(first, last + 1)
  const lowerOf = (i: number) => (first + i > 0 ? entries[first + i - 1][0] : '0')
  const max = Math.max(...shown.map(([, c]) => c))

  return (
    <div className="flex h-[180px] items-end gap-1 pt-2">
      {shown.map(([le, count], i) => (
        <div key={le} className="flex h-full min-w-0 flex-1 flex-col items-center justify-end gap-1">
          <span className="font-mono text-[10px] text-muted-foreground tabular-nums">
            {count > 0 ? formatCount(count) : ''}
          </span>
          <div
            className="w-full rounded-t bg-[var(--chart-1)]/70"
            style={{ height: `${(count / max) * 100}%`, minHeight: count > 0 ? 2 : 0 }}
            title={`${formatCount(count)} calls between ${formatSeconds(Number(lowerOf(i)))} and ${
              le === '+Inf' ? '∞' : formatSeconds(Number(le))
            }`}
          />
          <span className="w-full text-center font-mono text-[10px] whitespace-nowrap text-muted-foreground">
            {shortSeconds(le)}
          </span>
        </div>
      ))}
    </div>
  )
}
