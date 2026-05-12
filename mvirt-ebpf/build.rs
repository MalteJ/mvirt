fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../mvirt-net/proto/net.proto");
    tonic_prost_build::compile_protos("../mvirt-net/proto/net.proto")?;
    Ok(())
}
