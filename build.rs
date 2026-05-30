fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!(
            "cargo:rustc-link-arg-bin=codex-remote=/MANIFESTINPUT:packaging/windows/codex-remote.exe.manifest"
        );
        println!("cargo:rustc-link-arg-bin=codex-remote=/MANIFEST:EMBED");
        println!("cargo:rerun-if-changed=packaging/windows/codex-remote.rc");
        println!("cargo:rerun-if-changed=packaging/icons/AppIcon.ico");
        embed_resource::compile("packaging/windows/codex-remote.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
