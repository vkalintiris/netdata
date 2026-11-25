import { Route, Routes } from 'react-router'
import { AppShell } from '@/components/AppShell'
import { MetricsPage } from '@/pages/MetricsPage'
import { SpansPage } from '@/pages/SpansPage'
import { TracesPage } from '@/pages/TracesPage'

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<MetricsPage />} />
        <Route path="/spans" element={<SpansPage />} />
        <Route path="/traces" element={<TracesPage />} />
      </Routes>
    </AppShell>
  )
}
