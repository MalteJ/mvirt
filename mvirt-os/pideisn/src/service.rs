use crate::{log_error, log_info, log_warn};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub struct ServiceManager {
    services: Mutex<HashMap<String, ServiceState>>,
}

struct ServiceState {
    config: ServiceConfig,
    child: Option<Child>,
    restart_count: u32,
}

#[derive(Clone)]
pub struct ServiceConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub restart: bool,
    pub max_restarts: u32,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self {
            services: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, config: ServiceConfig) {
        let name = config.name.clone();
        let mut services = self.services.lock().await;
        services.insert(
            name,
            ServiceState {
                config,
                child: None,
                restart_count: 0,
            },
        );
    }

    pub async fn start(&self, name: &str) -> Result<(), String> {
        let mut services = self.services.lock().await;
        let state = services
            .get_mut(name)
            .ok_or_else(|| format!("Service {} not found", name))?;

        if state.child.is_some() {
            return Err(format!("Service {} already running", name));
        }

        let child = spawn_service(&state.config)?;
        state.child = Some(child);
        log_info!("Started service: {}", name);

        Ok(())
    }

    pub async fn start_all(&self) {
        let services: Vec<String> = {
            let services = self.services.lock().await;
            services.keys().cloned().collect()
        };

        for name in services {
            if let Err(e) = self.start(&name).await {
                log_error!("Failed to start {}: {}", name, e);
            }
        }
    }

    pub async fn check_and_restart(&self) {
        let mut services = self.services.lock().await;

        for (name, state) in services.iter_mut() {
            let should_restart = if let Some(ref mut child) = state.child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        log_warn!("Service {} exited with {}", name, status);
                        true
                    }
                    Ok(None) => false, // Still running
                    Err(e) => {
                        log_error!("Error checking service {}: {}", name, e);
                        false
                    }
                }
            } else {
                false
            };

            if should_restart && state.config.restart {
                state.child = None;

                if state.restart_count >= state.config.max_restarts {
                    log_error!(
                        "Service {} exceeded max restarts ({}), not restarting",
                        name,
                        state.config.max_restarts
                    );
                    continue;
                }

                state.restart_count += 1;
                log_info!(
                    "Restarting service {} (attempt {})",
                    name,
                    state.restart_count
                );

                match spawn_service(&state.config) {
                    Ok(child) => {
                        state.child = Some(child);
                    }
                    Err(e) => {
                        log_error!("Failed to restart {}: {}", name, e);
                    }
                }
            }
        }
    }
}

fn spawn_service(config: &ServiceConfig) -> Result<Child, String> {
    Command::new(&config.command)
        .args(&config.args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn {}: {}", config.command, e))
}

pub fn default_services() -> Vec<ServiceConfig> {
    vec![ServiceConfig {
        name: "mvirt-vmm".to_string(),
        command: "/usr/sbin/mvirt-vmm".to_string(),
        args: vec![],
        restart: true,
        max_restarts: 5,
    }]
}
