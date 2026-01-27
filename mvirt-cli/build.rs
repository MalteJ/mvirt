fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/mvirt.proto")?;
    tonic_prost_build::compile_protos("proto/zfs.proto")?;
    tonic_prost_build::compile_protos("proto/net.proto")?;
    Ok(())
}
