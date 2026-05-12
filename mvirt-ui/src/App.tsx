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
import { ProjectMembersPage } from './features/project/ProjectMembersPage'
import { ProjectServiceAccountsPage } from './features/project/ProjectServiceAccountsPage'
import { LogsPage } from './features/logs/LogsPage'
import {
  ClusterDetailPage,
  ClustersPage,
  OrgBillingPage,
  OrgDashboardPage,
  OrgLayout,
  OrgMembersPage,
  OrgsPage,
  OrgSettingsPage,
  ProjectsPage,
} from './features/admin'
import { WelcomePage } from './features/welcome'
import { useAuth } from './hooks/useAuth'
import { useProject } from './hooks/useProject'
import { useCluster, useNode, useOrgs, useProjects } from './hooks/queries'
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

// One-way URL → store sync. The URL is the single source of truth for
// scope; the zustand stores are convenience caches for components that
// would otherwise have to re-resolve the slug themselves (Sidebar nav,
// Header label, hooks like useLogs that key on currentProject.slug).
//
// Three URL shapes determine scope, in priority order:
//
//   1. `/projects/:projectSlug/*` → project scope
//      → set currentProject from URL slug; set currentOrg from
//        project's orgSlug.
//   2. `/orgs/:orgSlug/...` (any sub-path; bare `/orgs` is admin list)
//      → org scope
//      → set currentOrg from URL; clear currentProject.
//   3. anything else (welcome, `/orgs`, `/cluster`, …) → no scope
//      → clear both.
//
// Critical: this effect depends ONLY on the URL and the loaded
// orgs/projects lists — NOT on the current store values. Including the
// store values in the dep array creates a race: a state-setter call
// (e.g. from an action that's about to navigate) would re-run this
// effect with the OLD pathname and clear the just-set state before
// the navigation completes.
function ScopeSync() {
  const { pathname } = useLocation()
  const { data: orgs } = useOrgs()
  const { data: projects } = useProjects()
  const { setCurrentProject } = useProject()
  const { setCurrentOrg } = useOrg()

  useEffect(() => {
    const projectMatch = pathname.match(/^\/projects\/([^/]+)/)
    const orgMatch = pathname.match(/^\/orgs\/([^/]+)/)

    if (projectMatch) {
      const project = projects?.find((p) => p.slug === projectMatch[1])
      if (project) {
        setCurrentProject(project)
        const org = orgs?.find((o) => o.slug === project.orgSlug)
        if (org) setCurrentOrg(org)
      }
      return
    }

    setCurrentProject(null as unknown as never)

    if (orgMatch) {
      const org = orgs?.find((o) => o.slug === orgMatch[1])
      if (org) setCurrentOrg(org)
      return
    }

    setCurrentOrg(null)
  }, [pathname, orgs, projects, setCurrentProject, setCurrentOrg])

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

// Old `/clusters/:slug` URL — look up the cluster, bounce to the
// org-scoped variant. Bookmarks survive.
// Old `/cluster/:id` URL — look up the node and bounce to the org-scoped
// nodes detail. Keeps deep links from older sessions alive.
function LegacyNodeRedirect() {
  const { id } = useParams<{ id: string }>()
  const { data: node } = useNode(id ?? '')
  const { data: cluster } = useCluster(node?.clusterSlug)
  if (!id) return <Navigate to="/cluster" replace />
  if (!node || !cluster) return null
  return (
    <Navigate
      to={`/orgs/${cluster.orgSlug}/clusters/${cluster.slug}/nodes/${node.id}`}
      replace
    />
  )
}

function LegacyClusterRedirect() {
  const { slug } = useParams<{ slug: string }>()
  const { data: cluster } = useCluster(slug)
  if (!slug) return <Navigate to="/orgs" replace />
  if (!cluster) return null
  return <Navigate to={`/orgs/${cluster.orgSlug}/clusters/${slug}`} replace />
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
            <ScopeSync />
            <Layout>
              <Routes>
                <Route path="/" element={<WelcomePage />} />
                <Route path="/welcome" element={<WelcomePage />} />
                <Route path="/dashboard" element={<ProjectRedirect path="/dashboard" />} />
                <Route path="/cluster" element={<ClusterPage />} />
                <Route path="/cluster/:id" element={<LegacyNodeRedirect />} />
                <Route path="/orgs" element={<OrgsPage />} />
                {/* Org-scoped admin: nav lives in the global Sidebar,
                    each tab is its own route so deep links work. */}
                <Route path="/orgs/:orgSlug" element={<OrgLayout />}>
                  <Route index element={<Navigate to="dashboard" replace />} />
                  <Route path="dashboard" element={<OrgDashboardPage />} />
                  <Route path="projects" element={<ProjectsPage />} />
                  <Route path="clusters" element={<ClustersPage />} />
                  <Route
                    path="clusters/:slug"
                    element={<ClusterDetailPage />}
                  />
                  <Route
                    path="clusters/:slug/nodes/:nodeId"
                    element={<NodeDetailPage />}
                  />
                  <Route path="members" element={<OrgMembersPage />} />
                  <Route path="billing" element={<OrgBillingPage />} />
                  <Route path="settings" element={<OrgSettingsPage />} />
                </Route>
                {/* Legacy flat URL — bounce to the org-scoped path. */}
                <Route
                  path="/clusters/:slug"
                  element={<LegacyClusterRedirect />}
                />
                <Route path="/projects" element={<FlatProjectsRedirect />} />

                {/* Project-scoped routes — slug-based per ADR-0004.
                    ScopeSync (above) populates currentProject and
                    currentOrg from the URL; pages can read them directly. */}
                <Route
                  path="/projects/:projectSlug/*"
                  element={
                    <Routes>
                      <Route path="/dashboard" element={<DashboardPage />} />
                      <Route path="/vms" element={<VmsPage />} />
                      <Route path="/vms/new" element={<CreateVmPage />} />
                      <Route path="/vms/:id" element={<VmDetailPage />} />
                      <Route path="/storage" element={<StoragePage />} />
                      <Route path="/network" element={<NetworkPage />} />
                      <Route path="/firewall" element={<FirewallPage />} />
                      <Route path="/firewall/:id" element={<SecurityGroupDetailPage />} />
                      <Route path="/members" element={<ProjectMembersPage />} />
                      <Route
                        path="/service-accounts"
                        element={<ProjectServiceAccountsPage />}
                      />
                      <Route path="/logs" element={<LogsPage />} />
                    </Routes>
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
