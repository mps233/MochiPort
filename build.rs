fn main() {
    let build_number = std::env::var("THREADRELAY_BUILD_NUMBER")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=THREADRELAY_BUILD_NUMBER={build_number}");
    println!("cargo:rerun-if-env-changed=THREADRELAY_BUILD_NUMBER");

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
