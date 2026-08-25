// Build script for generating Rust code from protobuf definitions

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_files = [
        "proto/common.proto",
        "proto/capability.proto",
        "proto/node.proto",
        "proto/cell.proto",
        "proto/beacon.proto",
        "proto/composition.proto",
        "proto/zone.proto",
        "proto/role.proto",
        "proto/hierarchy.proto",
        "proto/command.proto",
        "proto/security.proto",
        "proto/track.proto",
        "proto/model.proto",
        "proto/sensor.proto",
        "proto/actuator.proto",
        "proto/effector.proto",
        "proto/product.proto",
        "proto/tasking.proto",
        "proto/event.proto",
        "proto/registry.proto",
        "proto/history.proto",
    ];

    // Configure prost to generate Rust code from .proto files
    let mut config = prost_build::Config::new();
    config
        .file_descriptor_set_path("proto/peat-schema-descriptor.bin")
        .skip_protoc_run();

    // Enable derive for common traits
    config.type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]");

    // Generate code from the checked-in descriptor set. Consumers do not need
    // a system protoc executable; maintainers regenerate the descriptor when a
    // source proto changes.
    config.compile_protos(&proto_files, &["proto/"])?;

    // Tell cargo to recompile if any proto file changes
    for file in &proto_files {
        println!("cargo:rerun-if-changed={}", file);
    }
    println!("cargo:rerun-if-changed=proto/peat-schema-descriptor.bin");

    Ok(())
}
