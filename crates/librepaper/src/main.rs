//! The binary. Everything is in the library beside it, so that `tools/test/fuzz/` can
//! link the parts of it that read untrusted input.

fn main() {
    librepaper::main()
}
