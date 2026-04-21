fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = tauri_build::WindowsAttributes::new();
        res = res.app_manifest(include_str!("rubypass.exe.manifest"));
        tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(res))
            .expect("failed to run tauri-build");
    }
    #[cfg(not(target_os = "windows"))]
    tauri_build::build()
}
