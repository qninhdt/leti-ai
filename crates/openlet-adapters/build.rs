//! Compile the vendored file-service proto into a tonic gRPC client for the
//! cloud `Filesystem` impl (`cloudfs`).
//!
//! The proto is a VENDORED COPY of openlet's `apps/file-service/proto/openlet/
//! file/v1/file.proto`, itself generated from TypeSpec. We only build the
//! client stubs (`build_server(false)`) — openlet-ai is a pure client of
//! file-service, never a server. `google/protobuf/timestamp.proto` resolves
//! via protoc's bundled well-known types.
//!
//! Deploy-ordering contract (Phase 6): this vendored proto must stay in sync
//! with the deployed file-service. The `GrepFiles` RPC it declares is only
//! served after the backend ships migration 000016 + the GrepFiles handler.
//! Until then a cloud-mode `grep` returns gRPC `Unimplemented`.

fn main() {
    let proto = "proto/openlet/file/v1/file.proto";
    println!("cargo:rerun-if-changed={proto}");

    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&[proto], &["proto"])
        .expect("compile file-service proto");
}
