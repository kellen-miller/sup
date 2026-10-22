fn main() {
    let mut arguments = std::env::args_os();
    let invoked_as = std::path::PathBuf::from(arguments.next().unwrap_or_default());
    let prompt = arguments.next().unwrap_or_else(|| "Password:".into());
    if let Err(error) = sup::auth::askpass(&invoked_as, &prompt.to_string_lossy()) {
        eprintln!("sup-askpass: {error}");
        std::process::exit(1);
    }
}
