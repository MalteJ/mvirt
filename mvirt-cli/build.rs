fn main() -> Result<(), Box<dyn std::error::Error>> {
    for p in ["proto/mvirt.proto", "proto/zfs.proto", "proto/net.proto"] {
        println!("cargo:rerun-if-changed={p}");
        tonic_prost_build::compile_protos(p)?;
    }
    Ok(())
}
