fn main() {
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .compile_protos(&["../../proto/quant_platform.proto"], &["../../proto"])
        .expect("failed to compile protobuf definitions");
}
