use std::{env, path::PathBuf};

fn main() {
    windows::build! {
        Windows::Win32::UI::WindowsAndMessaging::MessageBoxW,
    };

    println!("cargo:rustc-link-search={}/openvr/lib/win64", env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rustc-link-lib=openvr_api");
    println!("cargo:rerun-if-changed=openvr.h");

    let bindings = bindgen::Builder::default()
        .header("openvr.h")
        .clang_arg("-Iopenvr/headers")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Unable to generate openvr bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("openvr.rs"))
        .expect("Couldn't write bindings!");
}
