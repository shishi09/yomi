#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|a| a == "--self-test") {
        std::process::exit(yomi::run_self_test());
    }
    yomi::run();
}
