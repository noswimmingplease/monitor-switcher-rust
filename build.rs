fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    winresource::WindowsResource::new()
        .set_icon("assets/app.ico")
        .set_language(0x0809)
        .set("FileDescription", "Monitor Switcher")
        .set("ProductName", "Monitor Switcher")
        .set(
            "LegalCopyright",
            "Copyright © 2026 Monitor Switcher contributors; GPL-3.0-only",
        )
        .compile()
        .expect("Unable to embed the Windows application resources");
}
