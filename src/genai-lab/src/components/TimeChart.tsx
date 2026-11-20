import { useEffect, useRef } from 'react'
import uPlot from 'uplot'
import type { Series } from '@/lib/agent'
import { useTheme } from '@/lib/theme'

export type SeriesStyle = { label: string; color: string; name?: string }

type Props = {
  data: Series | undefined
  styles: SeriesStyle[]
  stacked?: boolean
  format: (v: number | null) => string
  height?: number
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

const hourMinuteSecond = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })

function timeTicks(u: uPlot, splits: number[]): string[] {
  const span = (u.scales.x.max ?? 0) - (u.scales.x.min ?? 0)
  const fmt = span > 2 * 86400 ? dayHour : span < 300 ? hourMinuteSecond : hourMinute
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

export function TimeChart({ data, styles, stacked, format, height = 180, onZoom, onPan, onReset }: Props) {
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

  const columns = (d: Series) => {
    const raw = present.map((s) => d.values[d.labels.indexOf(s.label)])
    rawRef.current = raw
    return [d.times, ...(stacked ? stack(raw) : raw)] as uPlot.AlignedData
  }

  // The chart is built once per shape and theme; new data only replaces its columns.
  useEffect(() => {
    const el = host.current
    const d = dataRef.current
    if (!el || !d) return

    const axis = cssVar('--chart-axis')
    const grid = cssVar('--chart-grid')
    const colors = present.map((s) => resolveColor(s.color))
    const initial = columns(d)

    const opts: uPlot.Options = {
      width: el.clientWidth,
      height,
      padding: [8, 8, 0, 0],
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
      },
      axes: [
        {
          stroke: axis,
          grid: { stroke: grid, width: 1 },
          ticks: { show: false },
          size: 24,
          space: 60,
          values: timeTicks,
        },
        {
          stroke: axis,
          grid: { stroke: grid, width: 1 },
          ticks: { show: false },
          size: 56,
          values: (_u, splits) => splits.map((v) => format(v)),
        },
      ],
      series: [
        {},
        ...present.map((s, i) => ({
          label: s.name ?? s.label,
          stroke: colors[i],
          width: 1.5,
          // Stacked layers fill the band down to the layer below (see bands), not down to zero.
          fill: stacked && i > 0 ? undefined : withAlpha(colors[i], stacked ? 0.35 : 0.12),
          points: {
            show: (u: uPlot, si: number) => hasIsolated(u.data[si] as (number | null)[]),
            size: 5,
            fill: colors[i],
          },
          spanGaps: false,
        })),
      ],
      bands: stacked
        ? present.slice(1).map((_s, i) => ({
            series: [i + 2, i + 1] as [number, number],
            fill: withAlpha(colors[i + 1], 0.35),
          }))
        : undefined,
      plugins: [
        panPlugin((sec) => handlers.current.onPan(sec)),
        tooltipPlugin(
          rawRef,
          present.map((s) => s.name ?? s.label),
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
  }, [ready, shape, stacked, format, height, theme])

  useEffect(() => {
    if (plot.current && data) plot.current.setData(columns(data))
  }, [data])

  return (
    <div className="flex min-w-0 flex-col">
      <div ref={host} className="relative w-full min-w-0" />
      <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 pl-14 text-xs text-muted-foreground">
        {present.map((s) => (
          <span key={s.label} className="flex items-center gap-1.5">
            <i className="inline-block h-[3px] w-3 rounded" style={{ background: `var(${s.color})` }} />
            {s.name ?? s.label}
          </span>
        ))}
      </div>
    </div>
  )
}
