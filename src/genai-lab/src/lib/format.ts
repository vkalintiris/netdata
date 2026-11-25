const compact = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 })
const plain = new Intl.NumberFormat('en', { maximumFractionDigits: 2 })

export function formatCount(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return '–'
  const rounded = Math.round(v)
  return Math.abs(rounded) >= 10_000 ? compact.format(rounded) : plain.format(rounded)
}

export function formatNumber(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return '–'
  return Math.abs(v) >= 10_000 ? compact.format(v) : plain.format(v)
}

export function formatSeconds(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return '–'
  if (v > 0 && v < 0.001) return `${plain.format(v * 1e6)} µs`
  if (v < 1) return `${plain.format(v * 1000)} ms`
  if (v < 60) return `${plain.format(v)} s`
  return `${plain.format(v / 60)} min`
}

export function formatPercent(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return '–'
  return `${plain.format(v * 100)}%`
}

export function formatInterval(seconds: number): string {
  if (seconds <= 0) return ''
  if (seconds < 60) return `${seconds} s`
  if (seconds < 3600) return `${plain.format(seconds / 60)} min`
  return `${plain.format(seconds / 3600)} h`
}

// US dollars; small sums keep two significant digits, since one model call can cost a fraction of a cent.
export function formatUsd(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return '–'
  if (v === 0) return '$0'
  if (Math.abs(v) < 0.01) return `$${Number(v.toPrecision(2))}`
  return `$${plain.format(v)}`
}
