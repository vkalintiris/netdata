import { Route, Routes } from 'react-router'
import { AppShell } from '@/components/AppShell'
import { BreakdownPage } from '@/pages/BreakdownPage'
import { LiteLLMPage } from '@/pages/LiteLLMPage'
import { MetricsPage } from '@/pages/MetricsPage'
import { TracePage } from '@/pages/TracePage'
import { TracesPage } from '@/pages/TracesPage'

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<MetricsPage />} />
        <Route path="/breakdown" element={<BreakdownPage />} />
        <Route path="/traces" element={<TracesPage />} />
        <Route path="/traces/:traceId" element={<TracePage />} />
        <Route path="/gateways/litellm" element={<LiteLLMPage />} />
      </Routes>
    </AppShell>
  )
}
