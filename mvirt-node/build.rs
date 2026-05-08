fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only node.proto is compiled here (the agent↔api wire format).
    // Daemon-side protos (vmm/zfs/net) come from the shared mvirt-daemon-protos crate.
    tonic_prost_build::configure()
        .build_server(false) // Client only
        .build_client(true)
        .compile_protos(&["../mvirt-api/proto/node.proto"], &["../mvirt-api/proto"])?;
    Ok(())
}
