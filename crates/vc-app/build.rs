//! Windows only: give the program its icon and version details (what
//! Explorer and the task bar show). Does nothing for other targets.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../packaging/windows/voice-changer.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../packaging/windows/voice-changer.ico");
    res.set("ProductName", "Voice Changer");
    res.set("FileDescription", "Voice Changer");
    res.set("CompanyName", "ZeroDay-Labz");
    res.set("LegalCopyright", "MIT License");
    // The program works without the resource: a missing resource compiler
    // (cross-checking from another system) must not stop the build.
    if let Err(e) = res.compile() {
        println!("cargo:warning=no icon or version details in the program: {e}");
    }
}
