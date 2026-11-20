import { Check } from 'lucide-react'
import { cn } from '@/lib/utils'

export type RankRow = { key: string; name: string; group?: string; value: number }

const LIMIT = 8
const UNSET = '[unset]'

export function Ranking({
  rows,
  format,
  selected,
  groupSelected,
  onSelect,
  empty,
}: {
  rows: RankRow[]
  format: (v: number) => string
  selected: string[]
  groupSelected?: string[]
  onSelect: (name: string, group?: string) => void
  empty: string
}) {
  const sorted = [...rows].filter((r) => r.value > 0).sort((a, b) => b.value - a.value)
  if (!sorted.length) return <p className="py-6 text-center text-sm text-muted-foreground">{empty}</p>
  const top = sorted.slice(0, LIMIT)
  const rest = sorted.slice(LIMIT)
  const max = top[0].value
  const restTotal = rest.reduce((s, r) => s + r.value, 0)

  return (
    <ul className="flex flex-col gap-0.5">
      {top.map((r) => {
        const active =
          selected.includes(r.name) && (!groupSelected || !r.group || groupSelected.includes(r.group))
        // The Agent reports a missing label as "[unset]", which is not a value a filter can match.
        const unset = r.name === UNSET || r.group === UNSET
        return (
          <li key={r.key}>
            <button
              type="button"
              disabled={unset}
              onClick={() => onSelect(r.name, r.group)}
              title={unset ? 'The label is missing on these calls' : active ? 'Remove this filter' : 'Filter the page by this value'}
              className={cn(
                'group relative flex w-full items-center gap-2 overflow-hidden rounded px-2 py-1 text-left text-sm hover:bg-muted disabled:cursor-default disabled:hover:bg-transparent',
                active && 'ring-1 ring-primary',
              )}
            >
              <span
                className="absolute inset-y-0 left-0 bg-primary/12"
                style={{ width: `${(r.value / max) * 100}%` }}
              />
              <span className="relative flex min-w-0 flex-1 items-center gap-1.5">
                {active && <Check className="size-3.5 shrink-0 text-primary" />}
                <span className="truncate">{r.name}</span>
                {r.group && <span className="shrink-0 text-xs text-muted-foreground">{r.group}</span>}
              </span>
              <span className="relative font-mono text-xs tabular-nums">{format(r.value)}</span>
            </button>
          </li>
        )
      })}
      {rest.length > 0 && (
        <li className="flex px-2 py-1 text-xs text-muted-foreground">
          <span className="flex-1">
            {rest.length} more
          </span>
          <span className="font-mono tabular-nums">{format(restTotal)}</span>
        </li>
      )}
    </ul>
  )
}
