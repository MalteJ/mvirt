import { Routes, Route, Navigate, useParams } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { LoginPage } from './features/auth/LoginPage'
import { TermsPage } from './features/auth/TermsPage'
import { DashboardPage } from './features/dashboard/DashboardPage'
import { ClusterPage } from './features/cluster/ClusterPage'
import { VmsPage } from './features/vms/VmsPage'
import { VmDetailPage } from './features/vms/VmDetailPage'
import { CreateVmPage } from './features/vms/CreateVmPage'
import { ContainersPage, CreatePodPage, PodDetailPage } from './features/containers'
import { StoragePage } from './features/storage/StoragePage'
import { NetworkPage } from './features/network/NetworkPage'
import { FirewallPage, SecurityGroupDetailPage } from './features/firewall'
import { LogsPage } from './features/logs/LogsPage'
import { ProjectsPage } from './features/admin'
import { useAuth } from './hooks/useAuth'
import { useProject } from './hooks/useProject'
import { useProjects } from './hooks/queries'
import { useEffect } from 'react'

function ProtectedRoute({ children }: { children: React.ReactNode }) {
  const { isAuthenticated } = useAuth()

  if (!isAuthenticated) {
    return <Navigate to="/login" replace />
  }

  return <>{children}</>
}

// Component to sync URL projectId with store
function ProjectSync({ children }: { children: React.ReactNode }) {
  const { projectId } = useParams<{ projectId: string }>()
  const { currentProject, setCurrentProject } = useProject()
  const { data: projects } = useProjects()

  useEffect(() => {
    if (projectId && projects) {
      const project = projects.find(p => p.id === projectId)
      if (project && (!currentProject || currentProject.id !== projectId)) {
        setCurrentProject(project)
      }
    }
  }, [projectId, projects, currentProject, setCurrentProject])

  return <>{children}</>
}

// Redirect to current project or first available project
function ProjectRedirect({ path }: { path: string }) {
  const { currentProject } = useProject()
  const { data: projects } = useProjects()

  if (currentProject) {
    return <Navigate to={`/p/${currentProject.id}${path}`} replace />
  }

  if (projects && projects.length > 0) {
    return <Navigate to={`/p/${projects[0].id}${path}`} replace />
  }

  // No projects, go to projects page
  return <Navigate to="/projects" replace />
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
                <Route path="/projects" element={<ProjectsPage />} />

                {/* Project-scoped routes */}
                <Route path="/p/:projectId/*" element={
                  <ProjectSync>
                    <Routes>
                      <Route path="/vms" element={<VmsPage />} />
                      <Route path="/vms/new" element={<CreateVmPage />} />
                      <Route path="/vms/:id" element={<VmDetailPage />} />
                      <Route path="/containers" element={<ContainersPage />} />
                      <Route path="/containers/new" element={<CreatePodPage />} />
                      <Route path="/containers/:id" element={<PodDetailPage />} />
                      <Route path="/storage" element={<StoragePage />} />
                      <Route path="/network" element={<NetworkPage />} />
                      <Route path="/firewall" element={<FirewallPage />} />
                      <Route path="/firewall/:id" element={<SecurityGroupDetailPage />} />
                      <Route path="/logs" element={<LogsPage />} />
                    </Routes>
                  </ProjectSync>
                } />

                {/* Redirects for old routes */}
                <Route path="/vms" element={<ProjectRedirect path="/vms" />} />
                <Route path="/vms/*" element={<ProjectRedirect path="/vms" />} />
                <Route path="/containers" element={<ProjectRedirect path="/containers" />} />
                <Route path="/containers/*" element={<ProjectRedirect path="/containers" />} />
                <Route path="/storage" element={<ProjectRedirect path="/storage" />} />
                <Route path="/network" element={<ProjectRedirect path="/network" />} />
                <Route path="/firewall" element={<ProjectRedirect path="/firewall" />} />
                <Route path="/firewall/*" element={<ProjectRedirect path="/firewall" />} />
                <Route path="/logs" element={<ProjectRedirect path="/logs" />} />
              </Routes>
            </Layout>
          </ProtectedRoute>
        }
      />
    </Routes>
  )
}

export default App
