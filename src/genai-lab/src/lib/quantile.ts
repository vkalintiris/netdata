// Percentiles from histogram buckets, interpolated within a bucket as Prometheus' histogram_quantile does.
// Buckets are per-bucket counts (not cumulative), keyed by their upper bound ("0.01" ... "+Inf").

export function quantile(q: number, buckets: Record<string, number>): number | null {
  const entries = Object.entries(buckets)
    .map(([le, count]) => ({ le: le === '+Inf' ? Infinity : Number(le), count }))
    .filter((b) => !Number.isNaN(b.le))
    .sort((a, b) => a.le - b.le)
  const total = entries.reduce((s, b) => s + b.count, 0)
  if (total <= 0) return null

  const rank = q * total
  let seen = 0
  let lower = 0
  for (const b of entries) {
    if (seen + b.count >= rank && b.count > 0) {
      // An answer in the open-ended bucket is reported as its lower bound.
      if (b.le === Infinity) return lower
      return lower + ((b.le - lower) * (rank - seen)) / b.count
    }
    seen += b.count
    if (b.le !== Infinity) lower = b.le
  }
  return lower
}
