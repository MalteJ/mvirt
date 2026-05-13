//! Generated proto bindings for `mvirt.node` (NodeAgent service hosted on the
//! node side of the reverse tunnel).

pub mod proto {
    // NodeEvent oneof variants intentionally vary in size — the larger
    // ones (e.g. VmStateChanged with an embedded mvirt.Vm) carry full
    // resource snapshots. Boxing buys a heap allocation per event with
    // no real benefit at our throughput.
    #![allow(clippy::large_enum_variant)]
    tonic::include_proto!("mvirt.node");
}
