# mvirt Control Plane Architecture

This document outlines the architectural patterns for `mvirt-cp`. We enforce a strict separation of concerns using the **Repository Pattern** (via the `DataStore` trait) backed by **Raft Consensus**.

## 1. Core Philosophy

1.  **The Log is the Database:** The authoritative source of truth is the Raft Log.
2.  **In-Memory State:** The working state (`CpState`) is a projection of the log, held entirely in memory for microsecond-latency reads.
3.  **Trait-Based Isolation:** API handlers never touch the Raft implementation or HashMaps directly. They interact exclusively with the `DataStore` trait.
4.  **Reactive:** Components subscribe to state changes via an Event Bus (Watch API).

## 2. System Topology

The system is layered. Higher layers rely only on the Interfaces of lower layers.

```mermaid
graph TD
    API[REST / gRPC Handlers]
    Trait[<< Trait >>\nDataStore]
    Impl[RaftStore]
    Mock[MockStore]
    
    State[CpState \n(In-Memory HashMap)]
    Consensus[mraft Node]
    Bus[Event Bus]

    API -->|dep| Trait
    Impl ..|>|impl| Trait
    Mock ..|>|impl| Trait
    
    Impl -->|read| State
    Impl -->|write| Consensus
    Consensus -->|apply| State
    State -.->|emit| Bus
    Bus -->|notify| Impl

```

## 3. The Programming Model

Developers interacting with the system state must use the `DataStore` interface.

### 3.1 The Store Trait ("The Law")

To ensure testability and decoupling, we use `async_trait`. The monolithic store is composed of smaller, domain-specific traits.

```rust
use async_trait::async_trait;

/// The central contract for accessing cluster state.
pub trait DataStore: NetworkStore + NicStore + SystemStore + Send + Sync {
    /// Subscribe to the global event stream for reactivity.
    fn watch(&self) -> broadcast::Receiver<Event>;
}

#[async_trait]
pub trait NetworkStore {
    async fn list_networks(&self) -> Result<Vec<Network>>;
    async fn get_network(&self, id: &str) -> Result<Option<Network>>;
    
    // Commands return the resulting state after the log is committed
    async fn create_network(&self, cmd: CreateNetworkRequest) -> Result<Network>;
    async fn delete_network(&self, id: &str, expected_version: Option<u64>) -> Result<()>;
}

#[async_trait]
pub trait NicStore {
    async fn list_nics(&self, network_filter: Option<&str>) -> Result<Vec<Nic>>;
    // ...
}

```

### 3.2 Consistency Model

We optimize for **Read Availability** and **Write Safety**.

* **Reads (Query):**
* **Mechanism:** Local Memory Read (`RwLock`).
* **Consistency:** Eventual / Sequential. A follower may be slightly behind the leader.
* **Pros:** Extremely fast (µs), works even if the leader is down (Read Availability).
* **Cons:** Could theoretically return stale data (ms range).


* **Writes (Command):**
* **Mechanism:** Raft Proposal.
* **Consistency:** Linearizable (Strong).
* **Flow:** Command is routed to Leader -> Replicated to Quorum -> Committed -> Applied.


* **Concurrency Control (OCC):**
* To prevent "Lost Updates" due to stale reads, critical mutations (Update/Delete) MUST support **Optimistic Concurrency Control**.
* Clients pass an `expected_version` (or `revision`). If the Raft state machine sees a mismatch during `apply()`, the command is rejected.



## 4. Implementation Patterns

### 4.1 The `RaftStore` Implementation

This struct bridges the async API world with the Raft consensus engine.

```rust
pub struct RaftStore {
    // The underlying Raft node handling consensus
    node: Arc<RwLock<RaftNode<Command, Response, CpState>>>,
    // Local event bus for the Watch API
    events: broadcast::Sender<Event>,
}

#[async_trait]
impl NetworkStore for RaftStore {
    async fn create_network(&self, req: CreateNetworkRequest) -> Result<Network> {
        // 1. Convert DTO to internal Command
        let cmd = Command::CreateNetwork { ... };
        
        // 2. Propose to Raft (blocks until committed)
        // write_or_forward handles routing to leader if we are a follower
        let response = self.node.write_or_forward(cmd).await?;
        
        // 3. Map Response to DTO
        match response {
            Response::Network(n) => Ok(n.into()),
            Response::Error { .. } => Err(Error::Business(...)),
            _ => Err(Error::Internal("Unexpected response")),
        }
    }
}

```

### 4.2 The State Machine (`CpState`)

The state machine logic must remain **deterministic**. It emits events via a side-channel to drive the reactive UI/Controllers.

```rust
impl StateMachine for CpState {
    fn apply(&mut self, cmd: Command) -> Response {
        // 1. Check version/revision for OCC
        if let Some(expected) = cmd.expected_version() {
             if self.get_version(cmd.id()) != expected {
                 return Response::Conflict;
             }
        }

        // 2. Apply mutation
        let (response, event) = self.process_logic(cmd);
        
        // 3. Emit Event (Side Effect)
        // This feeds the Store::watch() stream on ALL nodes (Leader & Followers)
        if let Some(event) = event {
             let _ = self.event_bus.send(event);
        }
        
        response
    }
}

```

## 5. Development Strategy

1. **Define Interfaces:** Start by defining `src/store/mod.rs` and the domain traits.
2. **Mocking:** Use `MockStore` for unit testing API handlers without spinning up a cluster.
3. **Async Traits:** Use `#[async_trait]` macro everywhere to handle the `dyn Trait` complexity.
4. **No Direct Access:** API Handlers (`src/rest/`) must only accept `Arc<dyn DataStore>`.

```

```