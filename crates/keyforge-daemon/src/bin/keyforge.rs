//! `keyforge` — the user facing CLI.

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let program = std::env::args()
        .next()
        .map(|p| {
            std::path::Path::new(&p)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "keyforge".to_string())
        })
        .unwrap_or_else(|| "keyforge".to_string());

    if argv.is_empty() {
        print!("{}", keyforge_daemon::cli::HELP);
        std::process::exit(2);
    }
    let code = keyforge_daemon::cli::main(argv, &program, None);
    std::process::exit(code);
}
