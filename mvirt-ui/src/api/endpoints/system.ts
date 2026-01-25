import { get } from '../client'
import type { SystemInfo } from '@/types'

export async function getSystemInfo(): Promise<SystemInfo> {
  return get<SystemInfo>('/system')
}
