use autotools;

fn main() {
    // Build and link IOHK libsodium
    let libsodium = autotools::Config::new("contrib/libsodium/").reconf("-vfi").build();
    println!("cargo:rustc-link-search=native={}", libsodium.join("lib").display());
    println!("cargo:rustc-link-lib=static=sodium");

    println!("cargo:return-if-changed=build.rs");
}
