//! PackELF - Executable packer for Linux ELF binaries
//!
//! A simple executable compressor inspired by UPX, but implemented in pure Rust
//! and supporting only Linux ELF executables with LZ4 compression.

mod elf;
mod packer;
mod stub;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "packelf")]
#[command(about = "Pack/unpack Linux ELF executables with LZ4 compression", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Pack (compress) an executable
    Pack {
        /// Input file to pack
        #[arg(value_name = "FILE")]
        input: PathBuf,

        /// Output file (defaults to <input>.packed)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,

        /// Force overwrite output if it exists
        #[arg(short, long)]
        force: bool,
    },
    /// Unpack (decompress) an executable
    Unpack {
        /// Input packed file
        #[arg(value_name = "FILE")]
        input: PathBuf,

        /// Output file (defaults to <input>.unpacked)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,

        /// Force overwrite output if it exists
        #[arg(short, long)]
        force: bool,
    },
    /// Show information about a packed executable
    Info {
        /// Packed file to inspect
        #[arg(value_name = "FILE")]
        input: PathBuf,
    },
    /// Test if a file is packed
    Test {
        /// File to test
        #[arg(value_name = "FILE")]
        input: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Pack {
            input,
            output,
            force,
        } => {
            let output = output.unwrap_or_else(|| {
                let mut path = input.clone();
                path.set_extension(format!(
                    "{}.packed",
                    path.extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("bin")
                ));
                path
            });

            if output.exists() && !force {
                anyhow::bail!(
                    "Output file '{}' already exists. Use --force to overwrite.",
                    output.display()
                );
            }

            println!("Packing {} -> {}", input.display(), output.display());
            packer::pack_file(&input, &output).context("Failed to pack file")?;

            let input_size = std::fs::metadata(&input)?.len();
            let output_size = std::fs::metadata(&output)?.len();
            let ratio = (output_size as f64 / input_size as f64) * 100.0;

            println!(
                "Success! {} bytes -> {} bytes ({:.1}% of original)",
                input_size, output_size, ratio
            );
        }
        Commands::Unpack {
            input,
            output,
            force,
        } => {
            let output = output.unwrap_or_else(|| {
                let mut path = input.clone();
                if let Some(ext) = path.extension() {
                    if ext == "packed" {
                        path.set_extension("");
                    } else {
                        path.set_extension(format!("{}.unpacked", ext.to_str().unwrap_or("bin")));
                    }
                } else {
                    path.set_extension("unpacked");
                }
                path
            });

            if output.exists() && !force {
                anyhow::bail!(
                    "Output file '{}' already exists. Use --force to overwrite.",
                    output.display()
                );
            }

            println!("Unpacking {} -> {}", input.display(), output.display());
            packer::unpack_file(&input, &output).context("Failed to unpack file")?;

            let input_size = std::fs::metadata(&input)?.len();
            let output_size = std::fs::metadata(&output)?.len();

            println!(
                "Success! {} bytes -> {} bytes",
                input_size, output_size
            );
        }
        Commands::Info { input } => {
            packer::show_info(&input).context("Failed to read file info")?;
        }
        Commands::Test { input } => {
            match packer::is_packed(&input) {
                Ok(true) => {
                    println!("{}: packed with PackELF", input.display());
                    std::process::exit(0);
                }
                Ok(false) => {
                    println!("{}: not packed", input.display());
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(2);
                }
            }
        }
    }

    Ok(())
}
