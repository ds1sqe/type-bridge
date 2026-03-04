//! CLI for generating Rust models from a TypeQL schema.
//!
//! Usage:
//!   type-bridge-codegen <input.tql> [output-dir]
//!   cat schema.tql | type-bridge-codegen - [output-dir]

use std::io::Read;
use std::path::PathBuf;
use std::{fs, process};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <input.tql | -> [output-dir]", args[0]);
        eprintln!();
        eprintln!("  <input.tql>  Path to a TypeQL define file, or '-' for stdin");
        eprintln!("  [output-dir] Directory to write generated files (default: ./models)");
        process::exit(1);
    }

    let input_path = &args[1];
    let output_dir = if args.len() >= 3 {
        PathBuf::from(&args[2])
    } else {
        PathBuf::from("models")
    };

    // Read input
    let input = if input_path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(|e| {
                eprintln!("Error reading stdin: {e}");
                process::exit(1);
            });
        buf
    } else {
        fs::read_to_string(input_path).unwrap_or_else(|e| {
            eprintln!("Error reading {input_path}: {e}");
            process::exit(1);
        })
    };

    // Generate
    let models = type_bridge_orm::codegen::generate_from_typeql(&input).unwrap_or_else(|e| {
        eprintln!("Error generating models: {e}");
        process::exit(1);
    });

    // Write output
    fs::create_dir_all(&output_dir).unwrap_or_else(|e| {
        eprintln!("Error creating output directory: {e}");
        process::exit(1);
    });

    let files = [
        ("mod.rs", &models.mod_rs),
        ("attributes.rs", &models.attributes_rs),
        ("entities.rs", &models.entities_rs),
        ("relations.rs", &models.relations_rs),
    ];

    for (name, content) in &files {
        let path = output_dir.join(name);
        fs::write(&path, content).unwrap_or_else(|e| {
            eprintln!("Error writing {}: {e}", path.display());
            process::exit(1);
        });
        eprintln!("  wrote {}", path.display());
    }

    eprintln!(
        "Done! Generated {} files in {}",
        files.len(),
        output_dir.display()
    );
}
