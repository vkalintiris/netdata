import { useQuery } from '@tanstack/react-query'
import { fetchSeries, type DataQuery } from './agent'

const REFRESH_MS = 10_000

// Relative windows follow "now"; an absolute (zoomed) window is fixed and is not refreshed.
export function useSeries(q: DataQuery | null) {
  return useQuery({
    queryKey: ['series', q],
    queryFn: ({ signal }) => fetchSeries(q!, signal),
    enabled: q != null,
    refetchInterval: q && q.window.after < 0 ? REFRESH_MS : false,
    placeholderData: (prev) => prev,
  })
}
