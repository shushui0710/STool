fn main() {
    // 必须自己声明对前端资源的依赖 —— `tauri-build` 只会为
    // `tauri.conf.json` 与 `capabilities/` 打 rerun-if-changed，
    // **不管 frontendDist**（见 `target/<profile>/build/stool-tauri-*/output`）。
    //
    // 后果很不显眼：改完 `src/js/*.js` 直接 `cargo build --release`，
    // 构建脚本不会重跑 → exe 里嵌的还是**上一版**前端。
    // 而内嵌资源是 brotli 压缩的，`grep -a <中文串> exe` 查不出任何东西，
    // 光看「编译成功」根本发现不了。判定办法只有比对时间戳：
    //   <profile>/build/stool-tauri-*/out/tauri-codegen-assets/  vs  src/js/
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=../preview.html");
    tauri_build::build();
}
