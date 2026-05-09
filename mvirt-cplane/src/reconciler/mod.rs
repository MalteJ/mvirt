//! API-side reconciliation controller.
//!
//! Subscribes to raft state-machine events and runs a periodic resync, then
//! dispatches per-resource reconcilers that issue daemon RPCs to the owning
//! node via the reverse tunnel and write status updates back to raft.
//!
//! Each per-resource module exposes the same two functions:
//! ```ignore
//! pub async fn reconcile(ctx: &Ctx, id: &str) -> anyhow::Result<()>;
//! pub fn list_ids(state: &ApiState) -> Vec<String>;
//! ```
//! Adding a new reconciler is: a new module under `reconciler/`, plus one
//! match arm in [`Controller::dispatch_event`] and one block in
//! [`Controller::resync_all`].

pub mod context;
pub mod network;
pub mod nic;
pub mod security_group;
pub mod template;
pub mod vm;
pub mod volume;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::audit::ApiAuditLogger;
use crate::store::{Event, RaftStore};
use crate::tunnel::NodeRegistry;

pub use context::Ctx;

const RESYNC_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Controller {
    ctx: Ctx,
}

impl Controller {
    pub fn new(
        store: Arc<RaftStore>,
        registry: Arc<NodeRegistry>,
        audit: Arc<ApiAuditLogger>,
    ) -> Self {
        Self {
            ctx: Ctx {
                store,
                registry,
                audit,
            },
        }
    }

    /// Spawn the controller loop. Returns immediately.
    pub fn spawn(self, mut events: broadcast::Receiver<Event>) {
        info!("starting reconciler controller (resync every {RESYNC_INTERVAL:?})");
        tokio::spawn(async move {
            let mut resync = tokio::time::interval(RESYNC_INTERVAL);
            resync.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Initial resync immediately
            resync.tick().await;
            self.resync_all().await;

            loop {
                tokio::select! {
                    _ = resync.tick() => self.resync_all().await,
                    msg = events.recv() => match msg {
                        Ok(event) => self.dispatch_event(event).await,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "controller lagged on event stream; forcing resync");
                            self.resync_all().await;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("event channel closed; controller stopping");
                            break;
                        }
                    }
                }
            }
        });
    }

    /// Dispatch a single state-machine event to the appropriate reconciler.
    /// To wire a new resource type: add a match arm here.
    async fn dispatch_event(&self, event: Event) {
        let kind = event.resource_type();
        let id = event.resource_id().to_string();
        debug!(kind, %id, "reconcile event");
        let ctx = &self.ctx;
        let result = match event {
            // Node lifecycle is owned by the tunnel acceptor, not reconcilers.
            Event::NodeRegistered(_)
            | Event::NodeUpdated { .. }
            | Event::NodeDeregistered { .. } => return,

            Event::NetworkCreated(_)
            | Event::NetworkUpdated { .. }
            | Event::NetworkDeleted { .. } => network::reconcile(ctx, &id).await,
            Event::NicCreated(_) | Event::NicUpdated { .. } | Event::NicDeleted { .. } => {
                nic::reconcile(ctx, &id).await
            }
            Event::VmCreated(_)
            | Event::VmUpdated { .. }
            | Event::VmStatusUpdated { .. }
            | Event::VmDeleted { .. } => vm::reconcile(ctx, &id).await,
            Event::VolumeCreated(_) | Event::VolumeDeleted { .. } => {
                volume::reconcile(ctx, &id).await
            }
            Event::TemplateCreated(_) | Event::TemplateUpdated { .. } => {
                template::reconcile(ctx, &id).await
            }
            Event::SecurityGroupCreated { .. } | Event::SecurityGroupDeleted { .. } => {
                security_group::reconcile(ctx, &id).await
            }
        };
        if let Err(e) = result {
            warn!(kind, %id, error = %e, "reconcile failed");
        }
    }

    /// Walk every reconciled resource type and call its reconciler. Anchors
    /// correctness against drift from missed events and newly-connected nodes.
    /// To wire a new resource type: add a block here.
    async fn resync_all(&self) {
        let state = self.ctx.store.snapshot().await;
        debug!(
            volumes = state.volumes.len(),
            vms = state.vms.len(),
            nics = state.nics.len(),
            networks = state.networks.len(),
            templates = state.templates.len(),
            security_groups = state.security_groups.len(),
            "resync"
        );
        let ctx = &self.ctx;
        for id in volume::list_ids(&state) {
            let _ = volume::reconcile(ctx, &id).await;
        }
        for id in vm::list_ids(&state) {
            let _ = vm::reconcile(ctx, &id).await;
        }
        for id in nic::list_ids(&state) {
            let _ = nic::reconcile(ctx, &id).await;
        }
        for id in network::list_ids(&state) {
            let _ = network::reconcile(ctx, &id).await;
        }
        for id in template::list_ids(&state) {
            let _ = template::reconcile(ctx, &id).await;
        }
        for id in security_group::list_ids(&state) {
            let _ = security_group::reconcile(ctx, &id).await;
        }
    }
}
