import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { listDatabases, getDatabase, createDatabase, deleteDatabase, startDatabase, stopDatabase } from '@/api/endpoints'
import type { CreateDatabaseRequest } from '@/types'

export const databaseKeys = {
  all: ['databases'] as const,
  lists: () => [...databaseKeys.all, 'list'] as const,
  list: () => [...databaseKeys.lists()] as const,
  details: () => [...databaseKeys.all, 'detail'] as const,
  detail: (id: string) => [...databaseKeys.details(), id] as const,
}

export function useDatabases() {
  return useQuery({
    queryKey: databaseKeys.list(),
    queryFn: listDatabases,
    refetchInterval: 5000,
  })
}

export function useDatabase(id: string) {
  return useQuery({
    queryKey: databaseKeys.detail(id),
    queryFn: () => getDatabase(id),
    enabled: !!id,
    refetchInterval: 2000,
  })
}

export function useCreateDatabase() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateDatabaseRequest) => createDatabase(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.lists() })
    },
  })
}

export function useDeleteDatabase() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deleteDatabase(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.lists() })
    },
  })
}

export function useStartDatabase() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => startDatabase(id),
    onSuccess: (db) => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.lists() })
      queryClient.setQueryData(databaseKeys.detail(db.id), db)
    },
  })
}

export function useStopDatabase() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => stopDatabase(id),
    onSuccess: (db) => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.lists() })
      queryClient.setQueryData(databaseKeys.detail(db.id), db)
    },
  })
}
