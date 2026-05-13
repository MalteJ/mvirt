import { get } from '../client'
import type { LogEntry, LogQueryRequest } from '@/types'

interface LogsResponse {
  logs: LogEntry[]
}

export async function queryLogs(request: LogQueryRequest): Promise<LogEntry[]> {
  const params = new URLSearchParams()
  if (request.objectId) params.set('objectId', request.objectId)
  if (request.limit) params.set('limit', request.limit.toString())

  const query = params.toString()
  const response = await get<LogsResponse>(`/logs${query ? `?${query}` : ''}`)
  return response.logs
}
