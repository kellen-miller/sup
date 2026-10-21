fn main() {
    std::process::exit(sup::cli::run(std::env::args_os().skip(1)));
}
