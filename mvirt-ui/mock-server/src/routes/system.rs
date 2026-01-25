use axum::Json;

use crate::state::SystemInfo;

pub async fn get_system_info() -> Json<SystemInfo> {
    Json(SystemInfo {
        version: "0.1.0".to_string(),
        hostname: "mvirt-host".to_string(),
        cpu_count: 8,
        memory_total_bytes: 32 * 1024 * 1024 * 1024, // 32GB
        memory_used_bytes: 12 * 1024 * 1024 * 1024,  // 12GB
        uptime: 86400 * 5 + 3600 * 12,               // 5 days, 12 hours
    })
}
