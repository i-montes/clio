//! Pi, the guard and the tool host, attacked as one stack. One test binary: the harness in
//! `common` launches Pi through the product's own `pi::launch::build`, so every scenario runs
//! inside the same `bwrap` jail the person's Pi runs in.
mod common;
mod failclosed;
mod latency;
mod stack;
