fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/mvirt.proto")?;
    tonic_build::compile_protos("proto/zfs.proto")?;
    tonic_build::compile_protos("proto/net.proto")?;
    Ok(())
}
