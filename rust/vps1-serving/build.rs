fn main() {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["../../proto/quant_platform.proto"], &["../../proto"])
        .expect("failed to compile protobuf definitions");
}
