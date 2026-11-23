fn main() {
    slint_build::compile("ui/app.slint").expect("Slint compilation failed");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("ui/assets/icon.ico");
        res.set("ProductName", "Riff");
        res.set("FileDescription", "Riff - Native Rust + Slint Audio Player");
        res.set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            println!("cargo:warning=winresource failed: {e}");
        }
    }
}
