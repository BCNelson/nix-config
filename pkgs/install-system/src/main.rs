use clap::Parser;

#[derive(Parser)]
struct Cli {
    host: String,
    user: String,
    target_disk: std::path::PathBuf,
}

fn main() {
    let args = Cli::parse();
    println!("Host: {}", args.host);
    println!("User: {}", args.user);
    println!("Target disk: {}", args.target_disk.display());
}
