import { Routes, Route, Navigate } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { DashboardPage } from './features/dashboard/DashboardPage'
import { VmsPage } from './features/vms/VmsPage'
import { VmDetailPage } from './features/vms/VmDetailPage'
import { StoragePage } from './features/storage/StoragePage'
import { NetworkPage } from './features/network/NetworkPage'
import { LogsPage } from './features/logs/LogsPage'

function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<Navigate to="/dashboard" replace />} />
        <Route path="/dashboard" element={<DashboardPage />} />
        <Route path="/vms" element={<VmsPage />} />
        <Route path="/vms/:id" element={<VmDetailPage />} />
        <Route path="/storage" element={<StoragePage />} />
        <Route path="/network" element={<NetworkPage />} />
        <Route path="/logs" element={<LogsPage />} />
      </Routes>
    </Layout>
  )
}

export default App
