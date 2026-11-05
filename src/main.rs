fn main() -> std::process::ExitCode {
    match rusty_make::cli::Cli::parse_ordered().run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
