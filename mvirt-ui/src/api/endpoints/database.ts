import { get, post, del } from '../client'
import type { Database, CreateDatabaseRequest, DatabaseListResponse } from '@/types'

export async function listDatabases(): Promise<Database[]> {
  const response = await get<DatabaseListResponse>('/databases')
  return response.databases
}

export async function getDatabase(id: string): Promise<Database> {
  return get<Database>(`/databases/${id}`)
}

export async function createDatabase(request: CreateDatabaseRequest): Promise<Database> {
  return post<Database>('/databases', request)
}

export async function deleteDatabase(id: string): Promise<void> {
  await del<void>(`/databases/${id}`)
}

export async function startDatabase(id: string): Promise<Database> {
  return post<Database>(`/databases/${id}/start`)
}

export async function stopDatabase(id: string): Promise<Database> {
  return post<Database>(`/databases/${id}/stop`)
}
