use std::{env, fs, process};

use type_bridge_core_lib::bindgen::{
    BindgenOptions, PythonRenderMetadata, TargetLanguage, generate_json_from_typeql,
};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: bindgen_render <schema.tql> <python|typescript|rust>");
        process::exit(2);
    }

    let input = fs::read_to_string(&args[1]).unwrap_or_else(|error| {
        eprintln!("failed to read {}: {error}", args[1]);
        process::exit(1);
    });
    let target: TargetLanguage = args[2].parse().unwrap_or_else(|error| {
        eprintln!("invalid target {}: {error}", args[2]);
        process::exit(1);
    });
    let options = BindgenOptions {
        schema_version: "1.0.0".to_string(),
        schema_filename: None,
        schema_text: Some(input.clone()),
        implicit_key_attributes: Vec::new(),
        python_metadata: PythonRenderMetadata::default(),
    };

    let rendered = generate_json_from_typeql(&input, target, &options).unwrap_or_else(|error| {
        eprintln!("render failed: {error}");
        process::exit(1);
    });
    println!("{rendered}");
}
