# Golden outputs

These files were captured from the pre-merge `jscx` and `rustcx` at commit
`0766e1b`. They are historical: they record what the old tools printed and must
never be regenerated. The tests in `crates/cmplx/tests/` compare the new
binaries against them, remapping the recorded fixture paths (`legacy/...`) to
the fixtures' current home under `crates/cmplx-{js,rust}/fixtures/`.
