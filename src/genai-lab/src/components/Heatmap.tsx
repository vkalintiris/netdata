import { useEffect, useRef, useState } from 'react'
import { formatCount } from '@/lib/format'
import { useTheme } from '@/lib/theme'

// One row of the heatmap: a duration range in seconds; `upper` null is the open-ended top bucket.
export type HeatRow = { lower: number; upper: number | null }

export type HeatmapData = {
  times: number[]
  step: number
  after: number
  before: number
  rows: HeatRow[]
  // counts[row][col], row 0 at the bottom
  counts: number[][]
}

export type HeatCell = { row: number; col: number }

type Props = {
  data: HeatmapData | undefined
  height?: number
  formatBound: (seconds: number) => string
  onCell?: (cell: HeatCell, data: HeatmapData) => void
  selected?: HeatCell | null
  cellHint?: string
}

const LEFT = 56
const BOTTOM = 22

const hourMinute = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
const dayHour = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: '2-digit' })
const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
const TICK_STEPS = [10, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400]

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

// Rows that hold no calls in the whole window are dropped from both ends, so the busy range gets the height.
function visibleRows(d: HeatmapData): [number, number] {
  const busy = d.rows.map((_, r) => d.counts[r]?.some((c) => c > 0) ?? false)
  const first = busy.indexOf(true)
  const last = busy.lastIndexOf(true)
  if (first < 0) return [0, -1]
  return [Math.max(0, first - 1), Math.min(d.rows.length - 1, last + 1)]
}

export function Heatmap({ data, height = 200, formatBound, onCell, selected, cellHint }: Props) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const box = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)
  const [hover, setHover] = useState<{ x: number; y: number; cell: HeatCell } | null>(null)
  const [theme] = useTheme()

  useEffect(() => {
    const el = box.current
    if (!el) return
    const ro = new ResizeObserver(() => setWidth(el.clientWidth))
    ro.observe(el)
    setWidth(el.clientWidth)
    return () => ro.disconnect()
  }, [])

  const range = data ? visibleRows(data) : ([0, -1] as [number, number])
  const rowCount = range[1] - range[0] + 1
  const plotW = Math.max(1, width - LEFT - 8)
  const plotH = height - BOTTOM - 4
  const span = data ? Math.max(1, data.before - data.after) : 1
  const xOf = (t: number) => LEFT + ((t - (data?.after ?? 0)) / span) * plotW
  const rowH = rowCount > 0 ? plotH / rowCount : plotH

  useEffect(() => {
    const c = canvas.current
    if (!c || !data || width === 0) return
    const dpr = window.devicePixelRatio || 1
    c.width = width * dpr
    c.height = height * dpr
    const ctx = c.getContext('2d')!
    ctx.scale(dpr, dpr)
    ctx.clearRect(0, 0, width, height)

    const axis = cssVar('--chart-axis')
    const grid = cssVar('--chart-grid')
    const color = cssVar('--chart-1')
    const font = `11px ${getComputedStyle(document.body).fontFamily}`
    ctx.font = font

    let max = 0
    for (let r = range[0]; r <= range[1]; r++) for (const v of data.counts[r] ?? []) max = Math.max(max, v)
    const cellW = Math.max(1, (data.step / span) * plotW)

    // Cells: a log scale keeps a few slow outliers visible next to the busy buckets.
    ctx.fillStyle = color
    for (let r = range[0]; r <= range[1]; r++) {
      const y = 4 + (range[1] - r) * rowH
      data.times.forEach((t, j) => {
        const v = data.counts[r]?.[j] ?? 0
        if (v <= 0) return
        ctx.globalAlpha = 0.15 + 0.85 * (Math.log1p(v) / Math.log1p(max))
        ctx.fillRect(xOf(t) + 0.5, y + 0.5, cellW - 1, rowH - 1)
      })
    }
    ctx.globalAlpha = 1

    if (selected && selected.row >= range[0] && selected.row <= range[1]) {
      ctx.strokeStyle = cssVar('--foreground')
      ctx.lineWidth = 1.5
      ctx.strokeRect(xOf(data.times[selected.col]), 4 + (range[1] - selected.row) * rowH, cellW, rowH)
    }

    // Left axis: every other row bound, so labels do not collide.
    ctx.fillStyle = axis
    ctx.textAlign = 'right'
    ctx.textBaseline = 'middle'
    const labelEvery = Math.max(1, Math.ceil(13 / rowH))
    for (let r = range[0]; r <= range[1]; r++) {
      if ((r - range[0]) % labelEvery) continue
      const row = data.rows[r]
      const y = 4 + (range[1] - r) * rowH
      ctx.fillText(row.upper == null ? `≥${formatBound(row.lower)}` : formatBound(row.upper), LEFT - 6, y + rowH / 2)
    }

    // Bottom axis: a round tick step that leaves about 110 px between labels.
    const want = span / Math.max(1, plotW / 110)
    const step = TICK_STEPS.find((s) => s >= want) ?? 86400
    const fmt = span > 12 * 3600 ? dayHour : hourMinute
    ctx.textAlign = 'center'
    ctx.textBaseline = 'top'
    ctx.strokeStyle = grid
    for (let t = Math.ceil(data.after / step) * step; t <= data.before; t += step) {
      const x = xOf(t)
      ctx.beginPath()
      ctx.moveTo(x, 4)
      ctx.lineTo(x, 4 + plotH)
      ctx.stroke()
      ctx.fillText(fmt.format(t * 1000), x, plotH + 8)
    }
  }, [data, width, height, theme, selected, range[0], range[1], rowH, plotW, plotH, span])

  const cellAt = (x: number, y: number): HeatCell | null => {
    if (!data || rowCount <= 0 || x < LEFT || y < 4 || y > 4 + plotH) return null
    const t = data.after + ((x - LEFT) / plotW) * span
    const col = data.times.findIndex((s) => t >= s && t < s + data.step)
    const row = range[1] - Math.floor((y - 4) / rowH)
    if (col < 0 || row < range[0] || row > range[1]) return null
    return { row, col }
  }

  const hovered = hover && data ? data.counts[hover.cell.row]?.[hover.cell.col] ?? 0 : 0
  const hoverRow = hover && data ? data.rows[hover.cell.row] : null

  return (
    <div ref={box} className="relative w-full" style={{ height }}>
      <canvas
        ref={canvas}
        style={{ width: '100%', height }}
        className={onCell ? 'cursor-pointer' : undefined}
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          const cell = cellAt(e.clientX - r.left, e.clientY - r.top)
          setHover(cell ? { x: e.clientX - r.left, y: e.clientY - r.top, cell } : null)
        }}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          const cell = cellAt(e.clientX - r.left, e.clientY - r.top)
          if (cell && data && onCell && (data.counts[cell.row]?.[cell.col] ?? 0) > 0) onCell(cell, data)
        }}
      />
      {data && rowCount <= 0 && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-muted-foreground">
          No data in this window
        </div>
      )}
      {hover && data && hoverRow && (
        <div
          className="chart-tip"
          style={
            hover.x > width / 2
              ? { right: width - hover.x + 12, top: Math.max(0, hover.y - 30) }
              : { left: hover.x + 12, top: Math.max(0, hover.y - 30) }
          }
        >
          <div className="chart-tip-time">
            {timeFmt.format(data.times[hover.cell.col] * 1000)} +{data.step}s
          </div>
          <div className="chart-tip-row">
            <span>
              {formatBound(hoverRow.lower)} – {hoverRow.upper == null ? '∞' : formatBound(hoverRow.upper)}
            </span>
            <b>{formatCount(hovered)} calls</b>
          </div>
          {onCell && hovered > 0 && cellHint && <div className="mt-1 text-[11px] text-muted-foreground">{cellHint}</div>}
        </div>
      )}
    </div>
  )
}
