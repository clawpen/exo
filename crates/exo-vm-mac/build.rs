use std::env;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    println!("cargo:rerun-if-changed=objc/ExoVMM.m");
    println!("cargo:rerun-if-changed=objc/ExoVMM.h");
    if target.contains("apple-darwin") {
        cc::Build::new()
            .file("objc/ExoVMM.m")
            .flag("-fobjc-arc")
            .compile("exovmm");
        println!("cargo:rustc-link-lib=framework=Virtualization");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
