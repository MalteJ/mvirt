fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(
            &[
                "../mvirt-vmm/proto/mvirt.proto",
                "../mvirt-zfs/proto/zfs.proto",
                "../mvirt-net/proto/net.proto",
            ],
            &[
                "../mvirt-vmm/proto",
                "../mvirt-zfs/proto",
                "../mvirt-net/proto",
            ],
        )?;
    Ok(())
}
