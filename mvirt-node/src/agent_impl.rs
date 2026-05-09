//! NodeAgent gRPC service implementation. Hosted by mvirt-node on the
//! reverse-tunnel socket and consumed by the api as a gRPC client.

use std::pin::Pin;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use crate::proto::node::node_agent_server::NodeAgent;
use crate::proto::{
    CurrentResourcesRequest, IdentifyRequest, IdentifyResponse, NodeEvent, NodeResources,
    WatchEventsRequest,
};

#[derive(Clone)]
pub struct NodeAgentService {
    pub node_id: String,
    pub name: String,
    pub address: String,
    pub resources: NodeResources,
    pub agent_version: String,
}

#[tonic::async_trait]
impl NodeAgent for NodeAgentService {
    type WatchEventsStream =
        Pin<Box<dyn Stream<Item = Result<NodeEvent, Status>> + Send + 'static>>;

    async fn identify(
        &self,
        _request: Request<IdentifyRequest>,
    ) -> Result<Response<IdentifyResponse>, Status> {
        Ok(Response::new(IdentifyResponse {
            node_id: self.node_id.clone(),
            name: self.name.clone(),
            address: self.address.clone(),
            resources: Some(self.resources),
            labels: Default::default(),
            agent_version: self.agent_version.clone(),
        }))
    }

    async fn watch_events(
        &self,
        _request: Request<WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        // Phase 1: empty stream that stays open. Phase 3 will drive real
        // events from local daemon notifications.
        let stream = futures::stream::pending();
        Ok(Response::new(Box::pin(stream)))
    }

    async fn current_resources(
        &self,
        _request: Request<CurrentResourcesRequest>,
    ) -> Result<Response<NodeResources>, Status> {
        Ok(Response::new(self.resources))
    }
}
