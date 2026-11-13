//! Stores the app icon in the Windows executable.
//!
//! The window icon set at run time only shows while the app is running.
//! Explorer, the Start menu, pinned shortcuts and the installer read the
//! icon from the executable itself.

fn main() {
    const ICON: &str = "../assets/icons/windows/rustydemon.ico";
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(ICON);
        resource
            .compile()
            .expect("embedding the icon in the executable");
    }
}
