import { get } from '../client'
import type { Me } from '@/types'

export async function getMe(): Promise<Me> {
  return get<Me>('/me')
}
