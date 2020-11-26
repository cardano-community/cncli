use std::process::Command;

macro_rules! ok (($expression:expr) => ($expression.unwrap()));
macro_rules! log {
    ($fmt:expr) => (println!(concat!("cncli/build.rs:{}: ", $fmt), line!()));
    ($fmt:expr, $($arg:tt)*) => (println!(concat!("cncli/build.rs:{}: ", $fmt),
    line!(), $($arg)*));
}

fn main() {
    run("git", |command| {
        command
            .arg("submodule")
            .arg("update")
            .arg("--init")
            .arg("--recursive")
    });
    run("git", |command| {
        command
            .arg("checkout")
            .arg("66f017f1")
            .current_dir("libsodium")
    });
    run("git", |command| {
        command
            .arg("reset")
            .arg("--hard")
            .arg("HEAD")
            .current_dir("libsodium")
    });

    run("./autogen.sh", |command| { command.current_dir("libsodium") });
    run("./configure", |command| { command.current_dir("libsodium") });
    run("make", |command| { command.current_dir("libsodium") });
    println!(r"cargo:rustc-link-search=libsodium/src/libsodium/.libs");
}

fn run<F>(name: &str, mut configure: F)
    where
        F: FnMut(&mut Command) -> &mut Command,
{
    let mut command = Command::new(name);
    let configured = configure(&mut command);
    log!("Executing {:?}", configured);
    if !ok!(configured.status()).success() {
        panic!("failed to execute {:?}", configured);
    }
    log!("Command {:?} finished successfully", configured);
}