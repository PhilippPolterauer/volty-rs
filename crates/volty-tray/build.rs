fn main() {
    println!("cargo:rerun-if-changed=volty.manifest");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winres::WindowsResource::new()
            .set_manifest_file("volty.manifest")
            .compile()
            .expect("failed to embed the Windows application manifest");
    }
}
