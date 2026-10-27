//! Binaire mince de développement. Le paquet installé est l'ELF de la fenêtre :
//! `griffe mcp` appelle [`griffe_mcp::run_stdio`].

fn main() {
    if let Err(err) = griffe_mcp::run_stdio() {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}
