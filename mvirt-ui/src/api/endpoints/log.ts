import { get } from '../client'
import type { LogEntry, LogQueryRequest } from '@/types'

export async function queryLogs(request: LogQueryRequest): Promise<LogEntry[]> {
  const params = new URLSearchParams()
  if (request.objectId) params.set('objectId', request.objectId)
  if (request.level) params.set('level', request.level)
  if (request.component) params.set('component', request.component)
  if (request.limit) params.set('limit', request.limit.toString())
  if (request.beforeId) params.set('beforeId', request.beforeId)

  const query = params.toString()
  const response = await get<{ entries: LogEntry[] }>(`/logs${query ? `?${query}` : ''}`)
  return response.entries
}
