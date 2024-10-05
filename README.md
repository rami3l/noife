# noife

This is a playground for me to tinker with ideas of addressing the (in)famous [rustup#988].

This repo contains several dummy executables to simulate an actual rustup-managed Rust installation.

This includes the executables of the Rust toolchain:

- `nargo`: A short-running (and possibly recursive) executable to simulate `nargo`.
- `nalyzer`: A long-running executable to simulate `rust-analyzer`.

... as well as the rustup-related components:

- `nustup`: A `rustup`-like executable that might run on its own,
  or serve as a proxy to other executables.
- `locker`: A companion service that is launched by `rustup` to ensure the actual locking
  semantics of the Rust installation in question.

  The design of this locker differs greatly from that of `cargo` due to rustup's dual nature
  and the fact that we have to take (possibly recursive) subprocesses in mind,
  given that there is nothing stopping our user from
  e.g. using `cargo run` to do some `rustup`-related action,
  which already gives a rather complicated call chain:

  > rustup's `cargo` shim -> `cargo run` -> `rustup toolchain`

[rustup#988]: https://github.com/rust-lang/rustup/issues/988
