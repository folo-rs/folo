fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(deprecated)] // OK, whatever, it works.
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .build_transport(true)
        .compile(&["proto/greet.proto"], &["proto"])?;

    Ok(())
}
