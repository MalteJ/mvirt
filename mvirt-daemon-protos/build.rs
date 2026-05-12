fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protos = [
        "../mvirt-vmm/proto/mvirt.proto",
        "../mvirt-zfs/proto/zfs.proto",
        "../mvirt-net/proto/net.proto",
    ];
    let includes = [
        "../mvirt-vmm/proto",
        "../mvirt-zfs/proto",
        "../mvirt-net/proto",
    ];
    for p in &protos {
        println!("cargo:rerun-if-changed={p}");
    }
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&protos, &includes)?;
    Ok(())
}
