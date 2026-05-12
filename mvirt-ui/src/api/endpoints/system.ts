import { get } from '../client'
import type { SystemInfo } from '@/types'

export async function getSystemInfo(): Promise<SystemInfo> {
  return get<SystemInfo>('/system')
}

export interface ApiVersion {
  version: string
}

/**
 * Liveness probe — hits the unauthenticated `/v1/version` endpoint and
 * resolves with the cplane version. Drives the connection indicator.
 */
export async function getApiVersion(): Promise<ApiVersion> {
  return get<ApiVersion>('/version')
}
