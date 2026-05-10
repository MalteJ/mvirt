import { getAccessToken } from '@/auth/userManager'

const API_BASE = (import.meta.env.VITE_API_URL ?? 'http://localhost:8080') + '/v1'

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

async function authHeaders(): Promise<Record<string, string>> {
  const token = await getAccessToken()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const errorText = await response.text()
    throw new ApiError(response.status, errorText || response.statusText)
  }
  if (response.status === 204 || response.headers.get('content-length') === '0') {
    return undefined as T
  }
  return response.json()
}

export async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    headers: await authHeaders(),
  })
  return handleResponse<T>(response)
}

export async function post<T, B = unknown>(path: string, body?: B): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders()),
    },
    body: body ? JSON.stringify(body) : undefined,
  })
  return handleResponse<T>(response)
}

export async function patch<T, B = unknown>(path: string, body?: B): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: 'PATCH',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders()),
    },
    body: body ? JSON.stringify(body) : undefined,
  })
  return handleResponse<T>(response)
}

export async function del<T>(path: string): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: 'DELETE',
    headers: await authHeaders(),
  })
  return handleResponse<T>(response)
}

// Stream endpoints stay unauthenticated for now — neither EventSource nor the
// browser WebSocket constructor accept custom headers, so a Bearer header isn't
// possible. Once the backend lands a JWT-aware fallback (e.g. `?access_token=`
// query param read from a one-time cookie), wire it up here.
export function createEventSource(path: string): EventSource {
  return new EventSource(`${API_BASE}${path}`)
}

export function createWebSocket(path: string): WebSocket {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return new WebSocket(`${protocol}//${window.location.host}${API_BASE}${path}`)
}
