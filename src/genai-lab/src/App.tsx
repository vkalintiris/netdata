import { Route, Routes } from 'react-router'
import { AppShell } from '@/components/AppShell'
import { CallsPage } from '@/pages/CallsPage'
import { MetricsPage } from '@/pages/MetricsPage'

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<MetricsPage />} />
        <Route path="/calls" element={<CallsPage />} />
      </Routes>
    </AppShell>
  )
}
