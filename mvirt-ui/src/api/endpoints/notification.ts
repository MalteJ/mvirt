import { get, post } from '../client'
import { Notification } from '@/types'

export async function getNotifications(): Promise<Notification[]> {
  return get('/notifications')
}

export async function markNotificationRead(id: string): Promise<void> {
  return post(`/notifications/${id}/read`)
}

export async function markAllNotificationsRead(): Promise<void> {
  return post('/notifications/read-all')
}
