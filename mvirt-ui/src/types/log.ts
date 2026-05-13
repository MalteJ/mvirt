export enum LogLevel {
  DEBUG = 'Debug',
  INFO = 'Info',
  WARN = 'Warn',
  ERROR = 'Error',
  AUDIT = 'Audit',
  NOTICE = 'Notice',
  CRITICAL = 'Critical',
  ALERT = 'Alert',
  EMERGENCY = 'Emergency',
}

export interface LogEntry {
  id: string
  timestamp: string
  message: string
  level: string
  component: string
  relatedObjectIds: string[]
}

export interface LogQueryRequest {
  objectId?: string
  limit?: number
}
