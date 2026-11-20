import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router'
import App from '@/App'
import { TooltipProvider } from '@/components/ui/tooltip'
import { BusyError } from '@/lib/traces'
import './index.css'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      // A busy Function is worth a few quick retries; other failures are shown at once.
      retry: (count, error) => (error instanceof BusyError ? count < 4 : count < 1),
      retryDelay: (count) => 300 * 2 ** count,
    },
  },
})

if (import.meta.env.DEV) (window as unknown as { __qc: QueryClient }).__qc = queryClient

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <TooltipProvider delayDuration={300}>
          <App />
        </TooltipProvider>
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
)
