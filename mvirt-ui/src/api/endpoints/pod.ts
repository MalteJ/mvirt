import { get, post, del } from '../client'
import type { Pod, CreatePodRequest, PodListResponse } from '@/types'

export async function listPods(): Promise<Pod[]> {
  const response = await get<PodListResponse>('/pods')
  return response.pods
}

export async function getPod(id: string): Promise<Pod> {
  return get<Pod>(`/pods/${id}`)
}

export async function createPod(request: CreatePodRequest): Promise<Pod> {
  return post<Pod>('/pods', request)
}

export async function deletePod(id: string): Promise<void> {
  await del<void>(`/pods/${id}`)
}

export async function startPod(id: string): Promise<Pod> {
  return post<Pod>(`/pods/${id}/start`)
}

export async function stopPod(id: string): Promise<Pod> {
  return post<Pod>(`/pods/${id}/stop`)
}
