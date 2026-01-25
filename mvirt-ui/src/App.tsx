import { Routes, Route, Navigate } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { LoginPage } from './features/auth/LoginPage'
import { TermsPage } from './features/auth/TermsPage'
import { DashboardPage } from './features/dashboard/DashboardPage'
import { ClusterPage } from './features/cluster/ClusterPage'
import { VmsPage } from './features/vms/VmsPage'
import { VmDetailPage } from './features/vms/VmDetailPage'
import { StoragePage } from './features/storage/StoragePage'
import { NetworkPage } from './features/network/NetworkPage'
import { LogsPage } from './features/logs/LogsPage'
import { useAuth } from './hooks/useAuth'

function ProtectedRoute({ children }: { children: React.ReactNode }) {
  const { isAuthenticated } = useAuth()

  if (!isAuthenticated) {
    return <Navigate to="/login" replace />
  }

  return <>{children}</>
}

function App() {
  const { isAuthenticated } = useAuth()

  return (
    <Routes>
      <Route
        path="/login"
        element={isAuthenticated ? <Navigate to="/dashboard" replace /> : <LoginPage />}
      />
      <Route path="/terms" element={<TermsPage />} />
      <Route
        path="/*"
        element={
          <ProtectedRoute>
            <Layout>
              <Routes>
                <Route path="/" element={<Navigate to="/dashboard" replace />} />
                <Route path="/dashboard" element={<DashboardPage />} />
                <Route path="/cluster" element={<ClusterPage />} />
                <Route path="/vms" element={<VmsPage />} />
                <Route path="/vms/:id" element={<VmDetailPage />} />
                <Route path="/storage" element={<StoragePage />} />
                <Route path="/network" element={<NetworkPage />} />
                <Route path="/logs" element={<LogsPage />} />
              </Routes>
            </Layout>
          </ProtectedRoute>
        }
      />
    </Routes>
  )
}

export default App
