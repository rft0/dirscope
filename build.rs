fn main() {
    println!("cargo:rerun-if-changed=assets/windows/dirscope.rc");
    println!("cargo:rerun-if-changed=assets/windows/dirscope.ico");

    // Embed the app icon and version info into the Windows .exe.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows/dirscope.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
