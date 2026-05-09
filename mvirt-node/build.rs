fn main() -> Result<(), Box<dyn std::error::Error>> {
    // node.proto: roles inverted across the reverse tunnel — the api is the
    // gRPC client, the node hosts the server.
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(
            &["../mvirt-cplane/proto/node.proto"],
            &["../mvirt-cplane/proto"],
        )?;
    Ok(())
}
