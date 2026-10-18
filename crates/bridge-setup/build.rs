use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-env-changed=BRIDGE_SETUP_PAYLOAD");
    let source = match env::var("BRIDGE_SETUP_PAYLOAD") {
        Ok(path) => {
            let path = fs::canonicalize(path).expect("BRIDGE_SETUP_PAYLOAD must exist");
            println!("cargo:rerun-if-changed={}", path.display());
            format!(
                "const PAYLOAD: &[u8] = include_bytes!({:?});",
                path.to_str().unwrap()
            )
        }
        Err(_) => "const PAYLOAD: &[u8] = &[];".into(),
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("payload.rs"),
        source,
    )
    .unwrap();
}
