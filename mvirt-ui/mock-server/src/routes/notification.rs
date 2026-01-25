use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationType {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    #[serde(rename = "type")]
    pub notification_type: NotificationType,
    pub title: String,
    pub message: String,
    pub read: bool,
    pub created_at: String,
}

pub async fn get_notifications() -> Json<Vec<Notification>> {
    let now = chrono::Utc::now();

    Json(vec![
        Notification {
            id: "notif-001".to_string(),
            notification_type: NotificationType::Success,
            title: "VM Started".to_string(),
            message: "web-server-01 has been started successfully".to_string(),
            read: false,
            created_at: (now - chrono::Duration::minutes(5)).to_rfc3339(),
        },
        Notification {
            id: "notif-002".to_string(),
            notification_type: NotificationType::Warning,
            title: "High Memory Usage".to_string(),
            message: "Node mvirt-node-02 is using 85% of available memory".to_string(),
            read: false,
            created_at: (now - chrono::Duration::minutes(15)).to_rfc3339(),
        },
        Notification {
            id: "notif-003".to_string(),
            notification_type: NotificationType::Info,
            title: "Snapshot Created".to_string(),
            message: "Automatic snapshot 'daily-2024-01-15' created for database-data".to_string(),
            read: true,
            created_at: (now - chrono::Duration::hours(2)).to_rfc3339(),
        },
        Notification {
            id: "notif-004".to_string(),
            notification_type: NotificationType::Error,
            title: "Import Failed".to_string(),
            message: "Template import from https://example.com/image.qcow2 failed: connection timeout".to_string(),
            read: true,
            created_at: (now - chrono::Duration::hours(5)).to_rfc3339(),
        },
        Notification {
            id: "notif-005".to_string(),
            notification_type: NotificationType::Info,
            title: "Node Joined".to_string(),
            message: "mvirt-node-03 has joined the cluster".to_string(),
            read: true,
            created_at: (now - chrono::Duration::days(1)).to_rfc3339(),
        },
    ])
}

pub async fn mark_notification_read() -> Json<()> {
    Json(())
}

pub async fn mark_all_read() -> Json<()> {
    Json(())
}
