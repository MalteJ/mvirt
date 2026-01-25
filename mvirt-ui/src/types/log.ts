export enum LogLevel {
  DEBUG = 'DEBUG',
  INFO = 'INFO',
  WARN = 'WARN',
  ERROR = 'ERROR',
  AUDIT = 'AUDIT',
}

export interface LogEntry {
  id: string
  timestampNs: number
  message: string
  level: LogLevel
  component: string
  relatedObjectIds: string[]
}

export interface LogQueryRequest {
  objectId?: string
  level?: LogLevel
  component?: string
  limit?: number
  beforeId?: string
}
