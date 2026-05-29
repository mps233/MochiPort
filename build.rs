fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!(
            "cargo:rustc-link-arg-bin=codex-remote-gui=/MANIFESTINPUT:packaging/windows/codex-remote-gui.exe.manifest"
        );
        println!("cargo:rustc-link-arg-bin=codex-remote-gui=/MANIFEST:EMBED");
        println!("cargo:rerun-if-changed=packaging/windows/codex-remote-gui.rc");
        println!("cargo:rerun-if-changed=packaging/icons/AppIcon.ico");
        embed_resource::compile(
            "packaging/windows/codex-remote-gui.rc",
            embed_resource::NONE,
        )
        .manifest_optional()
        .unwrap();
    }
}
