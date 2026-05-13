fn main() -> Result<(), Box<dyn std::error::Error>> {
    // node.proto imports the per-daemon protos (mvirt.proto / zfs.proto /
    // net.proto) so a NodeEvent can carry the full daemon-native resource
    // payload (Vm, Volume, etc) rather than a flattened projection.
    // We `extern_path` those packages to point at mvirt-daemon-protos so the
    // generated code references the existing types instead of redefining
    // them.
    let proto_includes = [
        "proto/",
        "../mvirt-vmm/proto",
        "../mvirt-zfs/proto",
        "../mvirt-net/proto",
    ];
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        // Specific message extern_paths (NOT package-prefix) — package
        // prefix `.mvirt` is greedy and would clobber our own `.mvirt.node`
        // package. Add more entries as new mvirt.* / mvirt.zfs.* /
        // mvirt.net.* types get embedded into NodeEvent variants.
        .extern_path(".mvirt.Vm", "::mvirt_daemon_protos::vmm::Vm")
        .extern_path(".mvirt.VmConfig", "::mvirt_daemon_protos::vmm::VmConfig")
        .extern_path(".mvirt.VmState", "::mvirt_daemon_protos::vmm::VmState")
        .extern_path(".mvirt.BootMode", "::mvirt_daemon_protos::vmm::BootMode")
        .extern_path(".mvirt.DiskConfig", "::mvirt_daemon_protos::vmm::DiskConfig")
        .extern_path(".mvirt.NicConfig", "::mvirt_daemon_protos::vmm::NicConfig")
        .compile_protos(&["proto/node.proto"], &proto_includes)?;

    println!("cargo:rerun-if-changed=proto/node.proto");
    println!("cargo:rerun-if-changed=../mvirt-vmm/proto/mvirt.proto");
    println!("cargo:rerun-if-changed=../mvirt-zfs/proto/zfs.proto");
    println!("cargo:rerun-if-changed=../mvirt-net/proto/net.proto");

    Ok(())
}
