fn main() {
    // 只有 desktop feature 才需要 tauri-build 生成 ACL / Windows 资源
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build();
    }
}
