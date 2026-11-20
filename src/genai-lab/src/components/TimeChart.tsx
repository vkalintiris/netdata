import { useEffect, useRef } from 'react'
import uPlot from 'uplot'
import type { Series } from '@/lib/agent'
import { useTheme } from '@/lib/theme'

// `right` puts a series on its own scale, for one that is orders of magnitude smaller than the others.
export type SeriesStyle = { label: string; color: string; name?: string; right?: boolean }

type Props = {
  data: Series | undefined
  styles: SeriesStyle[]
  stacked?: boolean
  format: (v: number | null) => string
  height?: number
  integer?: boolean
  onZoom: (after: number, before: number) => void
  onPan: (seconds: number) => void
  onReset: () => void
}

const SYNC_KEY = 'genai'

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

function resolveColor(color: string): string {
  return color.startsWith('--') ? cssVar(color) : color
}

const hourMinute = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
const dayHour = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: '2-digit' })
const INTEGER_INCRS = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1e3, 2e3, 5e3, 1e4, 2e4, 5e4, 1e5, 2e5, 5e5, 1e6, 1e7, 1e8, 1e9]

const hourMinuteSecond = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })

function timeTicks(u: uPlot, splits: number[]): string[] {
  const span = (u.scales.x.max ?? 0) - (u.scales.x.min ?? 0)
  const fmt = span > 12 * 3600 ? dayHour : span < 300 ? hourMinuteSecond : hourMinute
  return splits.map((t) => fmt.format(t * 1000))
}

// A value with no neighbours draws no line; such series get dots.
function hasIsolated(values: (number | null)[]): boolean {
  for (let i = 0; i < values.length; i++) {
    if (values[i] != null && values[i - 1] == null && values[i + 1] == null) return true
  }
  return false
}

function withAlpha(color: string, alpha: number): string {
  return `color-mix(in oklch, ${color} ${Math.round(alpha * 100)}%, transparent)`
}

// Stacked areas: each series is drawn on top of the ones before it; the legend keeps the raw values.
function stack(values: (number | null)[][]): (number | null)[][] {
  const out: (number | null)[][] = []
  const running: number[] = []
  values.forEach((row, i) => {
    out.push(
      row.map((v, j) => {
        running[j] = (i === 0 ? 0 : running[j]) + (v ?? 0)
        return v == null && i === 0 ? null : running[j]
      }),
    )
  })
  return out
}

// Shift + drag pans; a plain drag selects a window to zoom to (uPlot's own selection).
function panPlugin(onPan: (seconds: number) => void): uPlot.Plugin {
  return {
    hooks: {
      init: (u) => {
        const over = u.over
        let startX: number | null = null
        const down = (e: MouseEvent) => {
          if (!e.shiftKey) return
          e.stopPropagation()
          e.preventDefault()
          startX = e.clientX
          over.style.cursor = 'grabbing'
          window.addEventListener('mouseup', up, { once: true })
        }
        const up = (e: MouseEvent) => {
          if (startX == null) return
          const dx = e.clientX - startX
          startX = null
          over.style.cursor = ''
          const span = (u.scales.x.max ?? 0) - (u.scales.x.min ?? 0)
          const width = over.clientWidth || 1
          if (Math.abs(dx) > 3) onPan((-dx / width) * span)
        }
        u.root.addEventListener('mousedown', down, true)
      },
    },
  }
}

function tooltipPlugin(
  rawRef: { current: (number | null)[][] },
  names: string[],
  colors: string[],
  format: (v: number | null) => string,
): uPlot.Plugin {
  let tip: HTMLDivElement
  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
  return {
    hooks: {
      init: (u) => {
        tip = document.createElement('div')
        tip.className = 'chart-tip'
        tip.style.display = 'none'
        u.over.appendChild(tip)
        u.over.addEventListener('mouseleave', () => (tip.style.display = 'none'))
      },
      setCursor: (u) => {
        const idx = u.cursor.idx
        const left = u.cursor.left ?? -1
        // Charts that only follow another chart's cursor show the crosshair, not a tooltip.
        const local = (u.cursor as { event?: Event | null }).event != null
        if (idx == null || left < 0 || !local) {
          tip.style.display = 'none'
          return
        }
        const rows = names
          .map(
            (name, i) =>
              `<div class="chart-tip-row"><i style="background:${colors[i]}"></i><span>${name}</span><b>${format(
                rawRef.current[i]?.[idx] ?? null,
              )}</b></div>`,
          )
          .join('')
        tip.innerHTML = `<div class="chart-tip-time">${timeFmt.format(u.data[0][idx] * 1000)}</div>${rows}`
        tip.style.display = 'block'
        const flip = left > u.over.clientWidth / 2
        tip.style.left = flip ? '' : `${left + 12}px`
        tip.style.right = flip ? `${u.over.clientWidth - left + 12}px` : ''
        tip.style.top = '4px'
      },
    },
  }
}

function isEmpty(d: Series | undefined): boolean {
  return !d || d.values.every((row) => row.every((v) => v == null || v === 0))
}

export function TimeChart({ data, styles, stacked, format, height = 180, integer, onZoom, onPan, onReset }: Props) {
  const host = useRef<HTMLDivElement>(null)
  const plot = useRef<uPlot | null>(null)
  const handlers = useRef({ onZoom, onPan, onReset })
  handlers.current = { onZoom, onPan, onReset }
  const dataRef = useRef(data)
  dataRef.current = data
  const rawRef = useRef<(number | null)[][]>([])
  const [theme] = useTheme()

  const present = data ? styles.filter((s) => data.labels.includes(s.label)) : []
  const shape = present.map((s) => s.label).join('|')
  const ready = data != null

  // Stacked series are drawn as bars, highest layer first, so each lower layer paints over the part it owns.
  const ordered = stacked ? [...present].reverse() : present

  const columns = (d: Series) => {
    const raw = present.map((s) => d.values[d.labels.indexOf(s.label)])
    const drawn = stacked ? stack(raw).reverse() : raw
    rawRef.current = stacked ? [...raw].reverse() : raw
    return [d.times, ...drawn] as uPlot.AlignedData
  }

  // The chart is built once per shape and theme; new data only replaces its columns.
  useEffect(() => {
    const el = host.current
    const d = dataRef.current
    if (!el || !d) return

    const axis = cssVar('--chart-axis')
    const grid = cssVar('--chart-grid')
    const colors = ordered.map((s) => resolveColor(s.color))
    const bars = uPlot.paths.bars!({ size: [0.85, 48], gap: 1 })
    const initial = columns(d)
    const hasRight = !stacked && ordered.some((s) => s.right)
    const valueAxis = (scale: string, side: 1 | 3, color?: string): uPlot.Axis => ({
      scale,
      side,
      stroke: color ?? axis,
      grid: { show: side === 3, stroke: grid, width: 1 },
      ticks: { show: false },
      size: 56,
      incrs: integer ? INTEGER_INCRS : undefined,
      values: (_u, splits) => splits.map((v) => format(v)),
    })

    const opts: uPlot.Options = {
      width: el.clientWidth,
      height,
      padding: [8, hasRight ? 0 : 8, 0, 0],
      cursor: {
        sync: { key: SYNC_KEY, setSeries: false },
        drag: { x: true, y: false, setScale: false },
        y: false,
        points: { size: 6 },
      },
      legend: { show: false },
      scales: {
        x: {
          time: true,
          range: () => [dataRef.current?.after ?? 0, dataRef.current?.before ?? 0],
        },
        ...(hasRight ? { right: {} } : {}),
      },
      axes: [
        {
          stroke: axis,
          grid: { stroke: grid, width: 1 },
          ticks: { show: false },
          size: 24,
          // Labels with a date are wider and need more room between ticks.
          space: (_u, _i, min, max) => (max - min > 12 * 3600 ? 110 : 60),
          values: timeTicks,
        },
        valueAxis('y', 3),
        ...(hasRight ? [valueAxis('right', 1, colors[ordered.findIndex((s) => s.right)])] : []),
      ],
      series: [
        {},
        ...ordered.map((s, i) =>
          stacked
            ? {
                label: s.name ?? s.label,
                stroke: colors[i],
                width: 0,
                fill: withAlpha(colors[i], 0.8),
                paths: bars,
                points: { show: false },
              }
            : {
                label: s.name ?? s.label,
                scale: s.right ? 'right' : 'y',
                stroke: colors[i],
                width: 1.5,
                fill: withAlpha(colors[i], 0.12),
                points: {
                  show: (u: uPlot, si: number) => hasIsolated(u.data[si] as (number | null)[]),
                  size: 5,
                  fill: colors[i],
                },
                spanGaps: false,
              },
        ),
      ],
      plugins: [
        panPlugin((sec) => handlers.current.onPan(sec)),
        tooltipPlugin(
          rawRef,
          ordered.map((s) => s.name ?? s.label),
          colors,
          format,
        ),
      ],
      hooks: {
        setSelect: [
          (u) => {
            if (u.select.width < 4) return
            const after = u.posToVal(u.select.left, 'x')
            const before = u.posToVal(u.select.left + u.select.width, 'x')
            u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false)
            handlers.current.onZoom(Math.floor(after), Math.ceil(before))
          },
        ],
        ready: [
          (u) => {
            u.over.addEventListener('dblclick', () => handlers.current.onReset())
          },
        ],
      },
    }

    const u = new uPlot(opts, initial, el)
    plot.current = u
    const ro = new ResizeObserver(() => u.setSize({ width: el.clientWidth, height }))
    ro.observe(el)
    return () => {
      ro.disconnect()
      u.destroy()
      plot.current = null
    }
  }, [ready, shape, stacked, format, height, integer, theme])

  useEffect(() => {
    if (plot.current && data) plot.current.setData(columns(data))
  }, [data])

  return (
    <div className="flex min-w-0 flex-col">
      <div className="relative">
        <div ref={host} className="relative w-full min-w-0" />
        {ready && isEmpty(data) && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-muted-foreground">
            No data in this window
          </div>
        )}
      </div>
      <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 pl-14 text-xs text-muted-foreground">
        {present.map((s) => (
          <span key={s.label} className="flex items-center gap-1.5">
            <i className="inline-block h-[3px] w-3 rounded" style={{ background: `var(${s.color})` }} />
            {s.name ?? s.label}
            {s.right && !stacked && <span className="opacity-70">(right axis)</span>}
          </span>
        ))}
      </div>
    </div>
  )
}
