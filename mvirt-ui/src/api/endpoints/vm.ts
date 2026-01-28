import { get, post, del } from '../client'
import type { Vm, CreateVmRequest, VmListResponse } from '@/types'

export async function listVms(projectId: string): Promise<Vm[]> {
  const response = await get<VmListResponse>(`/projects/${projectId}/vms`)
  return response.vms
}

export async function getVm(id: string): Promise<Vm> {
  return get<Vm>(`/vms/${id}`)
}

export async function createVm(projectId: string, request: CreateVmRequest): Promise<Vm> {
  return post<Vm>(`/projects/${projectId}/vms`, request)
}

export async function deleteVm(id: string): Promise<void> {
  await del<void>(`/vms/${id}`)
}

export async function startVm(id: string): Promise<Vm> {
  return post<Vm>(`/vms/${id}/start`)
}

export async function stopVm(id: string): Promise<Vm> {
  return post<Vm>(`/vms/${id}/stop`)
}

export async function killVm(id: string): Promise<Vm> {
  return post<Vm>(`/vms/${id}/kill`)
}
