fn main() -> Result<(), Box<dyn std::error::Error>> {
    // node.proto: roles inverted across the reverse tunnel — the api is the
    // gRPC client, the node hosts the server. node.proto pulls in the
    // per-daemon protos for NodeEvent payloads; we extern_path those to
    // mvirt-daemon-protos to share types with the api side.
    println!("cargo:rerun-if-changed=../mvirt-cplane/proto/node.proto");
    println!("cargo:rerun-if-changed=../mvirt-vmm/proto/mvirt.proto");
    println!("cargo:rerun-if-changed=../mvirt-zfs/proto/zfs.proto");
    println!("cargo:rerun-if-changed=../mvirt-net/proto/net.proto");
    let includes = [
        "../mvirt-cplane/proto",
        "../mvirt-vmm/proto",
        "../mvirt-zfs/proto",
        "../mvirt-net/proto",
    ];
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .extern_path(".mvirt.Vm", "::mvirt_daemon_protos::vmm::Vm")
        .extern_path(".mvirt.VmConfig", "::mvirt_daemon_protos::vmm::VmConfig")
        .extern_path(".mvirt.VmState", "::mvirt_daemon_protos::vmm::VmState")
        .extern_path(".mvirt.BootMode", "::mvirt_daemon_protos::vmm::BootMode")
        .extern_path(".mvirt.DiskConfig", "::mvirt_daemon_protos::vmm::DiskConfig")
        .extern_path(".mvirt.NicConfig", "::mvirt_daemon_protos::vmm::NicConfig")
        .compile_protos(&["../mvirt-cplane/proto/node.proto"], &includes)?;
    Ok(())
}
