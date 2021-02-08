// macro_rules! ok (($expression:expr) => ($expression.unwrap()));
// macro_rules! log {
//     ($fmt:expr) => (println!(concat!("cncli/build.rs:{}: ", $fmt), line!()));
//     ($fmt:expr, $($arg:tt)*) => (println!(concat!("cncli/build.rs:{}: ", $fmt),
//     line!(), $($arg)*));
// }

fn main() {
    pkg_config::Config::new().probe("libsodium").unwrap();
    println!("cargo:return-if-changed=build.rs");
}
