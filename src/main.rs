#[cfg(not(target_arch = "wasm32"))]
mod parse;
#[cfg(not(target_arch = "wasm32"))]
mod runner;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    runner::main()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
