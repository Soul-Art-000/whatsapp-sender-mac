fn main() {
    // Tauri manifesti yalnızca masaüstü hedefinde gerekir; sunucu derlemesinde
    // tauri-build hiç çekilmez (Linux'ta WebKitGTK 4.0 istemeden derlenebilsin).
    #[cfg(feature = "desktop")]
    tauri_build::build();
}
