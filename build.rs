fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!(
            "cargo:rustc-link-arg-bin=codexhub=/MANIFESTINPUT:packaging/windows/codexhub.exe.manifest"
        );
        println!("cargo:rustc-link-arg-bin=codexhub=/MANIFEST:EMBED");
        println!("cargo:rerun-if-changed=packaging/windows/codexhub.rc");
        println!("cargo:rerun-if-changed=packaging/icons/AppIcon.ico");
        embed_resource::compile("packaging/windows/codexhub.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
