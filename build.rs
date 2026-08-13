fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!(
            "cargo:rustc-link-arg-bin=threadrelay=/MANIFESTINPUT:packaging/windows/threadrelay.exe.manifest"
        );
        println!("cargo:rustc-link-arg-bin=threadrelay=/MANIFEST:EMBED");
        println!("cargo:rerun-if-changed=packaging/windows/threadrelay.rc");
        println!("cargo:rerun-if-changed=packaging/icons/AppIcon.ico");
        embed_resource::compile("packaging/windows/threadrelay.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
