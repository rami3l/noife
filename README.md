# noife

This is a playground for me to tinker with ideas of addressing the (in)famous [rustup#988].

This repo contains several dummy executables to simulate an actual rustup-managed Rust installation:

- `nargo`: A short-running (and possibly recursive) executable to simulate `nargo`.
- `nalyzer`: A long-running executable to simulate `rust-analyzer`.
- `nustup`: A `rustup`-like executable that might run on its own, or serve as a proxy to other executables.

[rustup#988]: https://github.com/rust-lang/rustup/issues/988
