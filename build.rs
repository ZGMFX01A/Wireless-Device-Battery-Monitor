use std::env;
use std::fs;
use std::path::{Path, PathBuf};
mod version_format;

fn main() {
    println!("cargo:rerun-if-env-changed=APP_VERSION");
    println!("cargo:rerun-if-env-changed=GPUI_UPDATE_PUBLIC_KEY");
    println!("cargo:rerun-if-changed=version_format.rs");
    println!("cargo:rerun-if-changed=assets/app.ico");

    let raw_version = env::var("APP_VERSION")
        .unwrap_or_else(|_| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.3".to_string()));
    let version = version_format::normalize(&raw_version).expect("invalid APP_VERSION");
    if let Ok(key) = env::var("GPUI_UPDATE_PUBLIC_KEY") {
        assert!(
            key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid update public key"
        );
    }
    println!("cargo:rustc-env=APP_VERSION={version}");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let icon_path = Path::new(&manifest_dir).join("assets").join("app.ico");
        let icon_path_str = icon_path.to_string_lossy().replace('\\', "/");

        let parts: Vec<u32> = version
            .split('.')
            .filter_map(|s| s.split('-').next().and_then(|p| p.parse().ok()))
            .collect();
        let maj = parts.first().copied().unwrap_or(0);
        let min = parts.get(1).copied().unwrap_or(1);
        let pat = parts.get(2).copied().unwrap_or(0);

        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let rc_path = out_dir.join("app_generated.rc");

        let rc_content = format!(
            r#"#include <windows.h>
1 ICON "{icon_path_str}"
1 VERSIONINFO
 FILEVERSION {maj},{min},{pat},0
 PRODUCTVERSION {maj},{min},{pat},0
 FILEOS 0x40004
 FILETYPE 1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
 BLOCK "040904B0"
 BEGIN
 VALUE "FileDescription", "Wireless Device Battery Monitor\0"
 VALUE "FileVersion", "{maj}.{min}.{pat}.0\0"
 VALUE "ProductName", "Wireless Device Battery Monitor\0"
 VALUE "ProductVersion", "{version}\0"
 VALUE "OriginalFilename", "mouse-battery-gpui.exe\0"
 END
 END
 BLOCK "VarFileInfo"
 BEGIN
 VALUE "Translation", 0x409,1200
 END
END
"#
        );

        fs::write(&rc_path, rc_content).expect("write generated app.rc");
        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_required()
            .expect("embed application icon and dynamic version");
    }
}
