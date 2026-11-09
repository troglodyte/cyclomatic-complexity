use cmplx_core::language::Registry;
use std::process::ExitCode;

fn main() -> ExitCode {
    cmplx_core::run::run("cmplx", Registry::all(&[&cmplx_js::JS, &cmplx_rust::RUST]))
}
