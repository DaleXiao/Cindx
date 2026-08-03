fn main() {
    println!("cargo:rerun-if-env-changed=CINDX_SOURCE_REVISION");
    tauri_build::build();
}
