const API_BASE = '/v1'

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const errorText = await response.text()
    throw new ApiError(response.status, errorText || response.statusText)
  }
  return response.json()
}

export async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`)
  return handleResponse<T>(response)
}

export async function post<T, B = unknown>(path: string, body?: B): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: body ? JSON.stringify(body) : undefined,
  })
  return handleResponse<T>(response)
}

export async function del<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: 'DELETE',
  })
  return handleResponse<T>(response)
}

export function createEventSource(path: string): EventSource {
  return new EventSource(`${API_BASE}${path}`)
}

export function createWebSocket(path: string): WebSocket {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return new WebSocket(`${protocol}//${window.location.host}${API_BASE}${path}`)
}
