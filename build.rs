fn main() {
    let build_number = std::env::var("MOCHIPORT_DAEMON_BUILD_NUMBER")
        .or_else(|_| std::env::var("MOCHIPORT_BUILD_NUMBER"))
        .or_else(|_| std::env::var("THREADRELAY_BUILD_NUMBER"))
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=MOCHIPORT_DAEMON_BUILD_NUMBER={build_number}");
    println!("cargo:rerun-if-env-changed=MOCHIPORT_DAEMON_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=MOCHIPORT_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=THREADRELAY_BUILD_NUMBER");

    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!(
            "cargo:rustc-link-arg-bin=mochiport=/MANIFESTINPUT:packaging/windows/mochiport.exe.manifest"
        );
        println!("cargo:rustc-link-arg-bin=mochiport=/MANIFEST:EMBED");
        println!("cargo:rerun-if-changed=packaging/windows/mochiport.rc");
        println!("cargo:rerun-if-changed=packaging/icons/AppIcon.ico");
        embed_resource::compile("packaging/windows/mochiport.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
