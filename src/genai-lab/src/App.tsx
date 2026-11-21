import { Route, Routes } from 'react-router'
import { AppShell } from '@/components/AppShell'
import { CallsPage } from '@/pages/CallsPage'
import { OverviewPage } from '@/pages/OverviewPage'
import { SessionsPage } from '@/pages/SessionsPage'

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<OverviewPage />} />
        <Route path="/calls" element={<CallsPage />} />
        <Route path="/sessions" element={<SessionsPage />} />
      </Routes>
    </AppShell>
  )
}
