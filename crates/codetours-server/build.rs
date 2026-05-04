use std::process::Command;
use std::env;

fn main() {
    println!("cargo:rerun-if-changed=static/app.css");
    println!("cargo:rerun-if-changed=templates/");

    let out_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let input = format!("{}/static/app.css", out_dir);
    let output = format!("{}/static/app.generated.css", out_dir);

    let status = Command::new("tailwindcss")
        .args([
            "-i", &input,
            "-o", &output,
            "--minify",
        ])
        .status()
        .or_else(|_| {
            Command::new("../../tailwindcss")
                .args([
                    "-i", &input,
                    "-o", &output,
                    "--minify",
                ])
                .status()
        });

    match status {
        Ok(status) if status.success() => (),
        _ => {
            eprintln!("Error: tailwindcss CLI not found or failed.");
            eprintln!("Please install it via 'cargo binstall tailwindcss' or 'mise install'.");
            // In a real CI/Dev environment, we might want to panic here if it's mandatory.
            // For now, let's just ensure the file exists so include_str! doesn't fail if we are just checking types,
            // but the ticket says "Fail the build with a clear message".
            std::process::exit(1);
        }
    }
}
