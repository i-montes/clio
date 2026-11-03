//! `clio-toolhost`: see [`clio_toolhost::cli`] for the arguments.

fn main() {
    if let Err(e) = clio_toolhost::cli::run(std::env::args().skip(1).collect()) {
        eprintln!("clio-toolhost: {e}");
        std::process::exit(2);
    }
}
