fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile the node.proto from mvirt-api
    tonic_build::configure()
        .build_server(false) // Client only
        .build_client(true)
        .compile_protos(&["../mvirt-api/proto/node.proto"], &["../mvirt-api/proto"])?;
    Ok(())
}
