use std::process::ExitCode;

fn main() -> ExitCode {
    cmplx_core::run::run_as(&cmplx_js::JS)
}
