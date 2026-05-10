import { Routes, Route, Navigate, useLocation, useParams } from 'react-router-dom'
import { Layout } from './components/layout/Layout'
import { LoginPage } from './features/auth/LoginPage'
import { TermsPage } from './features/auth/TermsPage'
import { AuthCallback } from './features/auth/AuthCallback'
import { DashboardPage } from './features/dashboard/DashboardPage'
import { ClusterPage } from './features/cluster/ClusterPage'
import { NodeDetailPage } from './features/cluster/NodeDetailPage'
import { VmsPage } from './features/vms/VmsPage'
import { VmDetailPage } from './features/vms/VmDetailPage'
import { CreateVmPage } from './features/vms/CreateVmPage'
import { StoragePage } from './features/storage/StoragePage'
import { NetworkPage } from './features/network/NetworkPage'
import { FirewallPage, SecurityGroupDetailPage } from './features/firewall'
import { LogsPage } from './features/logs/LogsPage'
import { OrgsPage, ProjectsPage } from './features/admin'
import { WelcomePage } from './features/welcome'
import { useAuth } from './hooks/useAuth'
import { useProject } from './hooks/useProject'
import { useProjects } from './hooks/queries'
import { useOrg } from './hooks/useOrg'
import { useEffect } from 'react'

function ProtectedRoute({ children }: { children: React.ReactNode }) {
  const { isAuthenticated, isLoading } = useAuth()

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background text-muted-foreground">
        Loading…
      </div>
    )
  }

  if (!isAuthenticated) {
    return <Navigate to="/login" replace />
  }

  return <>{children}</>
}

// Sync the URL slug into the project store so pages downstream can read
// `useProject().currentProject` without re-resolving the slug themselves.
function ProjectSync({ children }: { children: React.ReactNode }) {
  const { projectSlug } = useParams<{ projectSlug: string }>()
  const { currentProject, setCurrentProject } = useProject()
  const { data: projects } = useProjects()

  useEffect(() => {
    if (projectSlug && projects) {
      const project = projects.find((p) => p.slug === projectSlug)
      if (project && (!currentProject || currentProject.slug !== projectSlug)) {
        setCurrentProject(project)
      }
    }
  }, [projectSlug, projects, currentProject, setCurrentProject])

  return <>{children}</>
}

// Mounted at the protected-route root so it runs on every route change.
// Clears the active-project store when the URL leaves `/projects/:slug/*` —
// without this, `currentProject` from a previous project visit lingers and
// downstream consumers (label, pod-list query, log query, …) think a
// project is active in the middle of an Org-scoped page.
function ProjectScopeSync() {
  const { pathname } = useLocation()
  const { currentProject, setCurrentProject } = useProject()
  useEffect(() => {
    const inProjectScope = /^\/projects\//.test(pathname)
    if (!inProjectScope && currentProject) {
      // setCurrentProject's typed signature requires a Project; pass null via
      // a cast — the store happily accepts it and downstream code already
      // checks for null/undefined.
      setCurrentProject(null as unknown as never)
    }
  }, [pathname, currentProject, setCurrentProject])
  return null
}

// Redirect to the active project (or first one in the active Org). Falls
// through to the Org's project-list if no project exists, or `/orgs` if
// there isn't even an Org yet.
function ProjectRedirect({ path }: { path: string }) {
  const { currentProject } = useProject()
  const { currentOrg } = useOrg()
  const { data: projects } = useProjects()

  if (currentProject) {
    return <Navigate to={`/projects/${currentProject.slug}${path}`} replace />
  }

  // Prefer projects within the active Org if one is set.
  const candidates = currentOrg
    ? projects?.filter((p) => p.orgSlug === currentOrg.slug)
    : projects

  if (candidates && candidates.length > 0) {
    return <Navigate to={`/projects/${candidates[0].slug}${path}`} replace />
  }

  if (currentOrg) {
    return <Navigate to={`/orgs/${currentOrg.slug}/projects`} replace />
  }
  return <Navigate to="/orgs" replace />
}

// Old `/projects` URL — bounce to the org-scoped variant for the active Org,
// or /orgs if there is none. Kept as a soft redirect so bookmarks survive.
function FlatProjectsRedirect() {
  const { currentOrg } = useOrg()
  return (
    <Navigate
      to={currentOrg ? `/orgs/${currentOrg.slug}/projects` : '/orgs'}
      replace
    />
  )
}

// Backward-compat redirect: old `/p/:projectId/*` URLs (UUID-based) bounce to
// the new slug-based shape. Bookmarks keep working for one cycle.
function LegacyProjectRedirect() {
  const { projectId, '*': rest = '' } = useParams<{ projectId: string; '*': string }>()
  const { data: projects } = useProjects()

  if (!projectId || !projects) {
    return null
  }
  const project = projects.find((p) => p.slug === projectId || p.slug === projectId)
  if (!project) {
    return <Navigate to="/projects" replace />
  }
  return <Navigate to={`/projects/${project.slug}${rest ? '/' + rest : ''}`} replace />
}

function App() {
  const { isAuthenticated } = useAuth()

  return (
    <Routes>
      <Route
        path="/login"
        element={isAuthenticated ? <Navigate to="/dashboard" replace /> : <LoginPage />}
      />
      <Route path="/auth/callback" element={<AuthCallback />} />
      <Route path="/terms" element={<TermsPage />} />
      <Route
        path="/*"
        element={
          <ProtectedRoute>
            <ProjectScopeSync />
            <Layout>
              <Routes>
                <Route path="/" element={<WelcomePage />} />
                <Route path="/welcome" element={<WelcomePage />} />
                <Route path="/dashboard" element={<ProjectRedirect path="/dashboard" />} />
                <Route path="/cluster" element={<ClusterPage />} />
                <Route path="/cluster/:id" element={<NodeDetailPage />} />
                <Route path="/orgs" element={<OrgsPage />} />
                <Route
                  path="/orgs/:orgSlug/projects"
                  element={<ProjectsPage />}
                />
                <Route path="/projects" element={<FlatProjectsRedirect />} />

                {/* Project-scoped routes — slug-based per ADR-0004 */}
                <Route
                  path="/projects/:projectSlug/*"
                  element={
                    <ProjectSync>
                      <Routes>
                        <Route path="/dashboard" element={<DashboardPage />} />
                        <Route path="/vms" element={<VmsPage />} />
                        <Route path="/vms/new" element={<CreateVmPage />} />
                        <Route path="/vms/:id" element={<VmDetailPage />} />
                        <Route path="/storage" element={<StoragePage />} />
                        <Route path="/network" element={<NetworkPage />} />
                        <Route path="/firewall" element={<FirewallPage />} />
                        <Route path="/firewall/:id" element={<SecurityGroupDetailPage />} />
                        <Route path="/logs" element={<LogsPage />} />
                      </Routes>
                    </ProjectSync>
                  }
                />

                {/* Backward-compat: old /p/:projectId/* paths redirect to /projects/:slug/*. */}
                <Route path="/p/:projectId/*" element={<LegacyProjectRedirect />} />

                {/* Bare-path redirects pick a default project. */}
                <Route path="/vms" element={<ProjectRedirect path="/vms" />} />
                <Route path="/vms/*" element={<ProjectRedirect path="/vms" />} />
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
