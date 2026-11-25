import { Route, Routes } from 'react-router'
import { AppShell } from '@/components/AppShell'
import { CallsPage } from '@/pages/CallsPage'
import { MetricsPage } from '@/pages/MetricsPage'
import { RunsPage } from '@/pages/RunsPage'

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<MetricsPage />} />
        <Route path="/calls" element={<CallsPage />} />
        <Route path="/runs" element={<RunsPage />} />
      </Routes>
    </AppShell>
  )
}
