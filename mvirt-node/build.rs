fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile proto files for all services mvirt-node connects to:
    // - node.proto: API server (registration, heartbeat, spec streaming, status reporting)
    // - mvirt.proto: mvirt-vmm (VM management)
    // - zfs.proto: mvirt-zfs (volume/template management)
    // - net.proto: mvirt-net (network/NIC/security group management)
    tonic_build::configure()
        .build_server(false) // Client only
        .build_client(true)
        .compile_protos(
            &[
                "../mvirt-api/proto/node.proto",
                "../mvirt-vmm/proto/mvirt.proto",
                "../mvirt-zfs/proto/zfs.proto",
                "../mvirt-net/proto/net.proto",
            ],
            &[
                "../mvirt-api/proto",
                "../mvirt-vmm/proto",
                "../mvirt-zfs/proto",
                "../mvirt-net/proto",
            ],
        )?;
    Ok(())
}
