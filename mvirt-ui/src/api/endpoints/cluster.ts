import { get, post, del } from '../client'
import { Node, ClusterInfo } from '@/types'

export async function getClusterInfo(): Promise<ClusterInfo> {
  return get('/cluster')
}

export async function getNodes(): Promise<Node[]> {
  return get('/cluster/nodes')
}

export async function getNode(id: string): Promise<Node> {
  return get(`/cluster/nodes/${id}`)
}

export async function setNodeMaintenance(id: string, maintenance: boolean): Promise<Node> {
  return post(`/cluster/nodes/${id}/maintenance`, { maintenance })
}

export async function removeNode(id: string): Promise<void> {
  return del(`/cluster/nodes/${id}`)
}
