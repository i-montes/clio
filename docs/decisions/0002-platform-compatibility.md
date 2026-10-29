# 0002: Platform compatibility

## Decision

- Everything platform-specific is isolated and marked, so supporting another OS later means adding code beside it, never rewriting shared code. Linux is the only target for now.
- Shared code never touches OS APIs, paths or commands directly. It goes through a small platform interface (a trait in the crate that needs it); each OS implements it.
- Marking, instead of a dedicated folder:
  - Rust: platform code lives in a module named after the OS (`linux.rs`, `macos.rs`, `windows.rs`) behind `#[cfg(target_os = "...")]`, next to the interface it implements.
  - Everything else (scripts, service units, packaging in `infra/`): the OS is part of the file name, e.g. `install.linux.sh`, `clio.linux.service`.
- A platform-specific file is easy to find with `find . -name '*linux*'` and has no name that hides which OS it is for.
- A target without an implementation fails at compile time (`compile_error!`), not at runtime.
- CI and `just` recipes that only make sense on one OS say so in their name or comment.
