fn main() {
    println!("cargo:rerun-if-changed=volty.manifest");
    println!("cargo:rerun-if-changed=assets/volty.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_manifest_file("volty.manifest")
            .set_icon("assets/volty.ico")
            .compile()
            .expect("failed to embed the Windows application manifest and icon");
    }
}
