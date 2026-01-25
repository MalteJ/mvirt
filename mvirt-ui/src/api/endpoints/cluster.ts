import { apiClient } from '../client'
import { Node, ClusterInfo } from '@/types'

export async function getClusterInfo(): Promise<ClusterInfo> {
  return apiClient.get('/cluster')
}

export async function getNodes(): Promise<Node[]> {
  return apiClient.get('/cluster/nodes')
}

export async function getNode(id: string): Promise<Node> {
  return apiClient.get(`/cluster/nodes/${id}`)
}

export async function setNodeMaintenance(id: string, maintenance: boolean): Promise<Node> {
  return apiClient.post(`/cluster/nodes/${id}/maintenance`, { maintenance })
}

export async function removeNode(id: string): Promise<void> {
  return apiClient.delete(`/cluster/nodes/${id}`)
}
