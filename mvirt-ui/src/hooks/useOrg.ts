import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { Org } from '@/types'

interface OrgState {
  currentOrg: Org | null
  setCurrentOrg: (org: Org | null) => void
}

/**
 * Zustand store for the active Org. Mirrors `useProject` and persists across
 * reloads. The store is the source of truth for "which Org is the user
 * currently looking at"; route-level slugs are derived from the store, not
 * the other way around.
 */
export const useOrg = create<OrgState>()(
  persist(
    (set) => ({
      currentOrg: null,
      setCurrentOrg: (org) => set({ currentOrg: org }),
    }),
    {
      name: 'mvirt-org',
    },
  ),
)
