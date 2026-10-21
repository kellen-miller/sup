fn main() {
    let prompt = std::env::args_os()
        .nth(1)
        .unwrap_or_else(|| "Password:".into());
    if let Err(error) = sup::auth::askpass(&prompt.to_string_lossy()) {
        eprintln!("sup-askpass: {error}");
        std::process::exit(1);
    }
}
