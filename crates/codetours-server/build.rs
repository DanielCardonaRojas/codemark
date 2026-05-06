use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=static/app.css");
    println!("cargo:rerun-if-changed=templates/");

    let out_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let input = format!("{}/static/app.css", out_dir);
    let output = format!("{}/static/app.generated.css", out_dir);

    let skip_tailwind = env::var("SKIP_TAILWIND_BUILD").unwrap_or_default() == "1";
    if skip_tailwind {
        println!("cargo:warning=SKIP_TAILWIND_BUILD is set, skipping regeneration.");
        return;
    }

    let status = Command::new("tailwindcss")
        .args(["-i", &input, "-o", &output, "--minify"])
        .status()
        .or_else(|_| {
            Command::new("../../tailwindcss")
                .args(["-i", &input, "-o", &output, "--minify"])
                .status()
        });

    match status {
        Ok(status) if status.success() => (),
        Ok(status) => {
            eprintln!("Fatal: tailwindcss exited with status {status}.");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("Error: tailwindcss CLI not found or failed.");
            eprintln!("Details: {err}");
            eprintln!("Please install it via 'cargo binstall tailwindcss' or 'mise install'.");
            // If the generated file already exists (e.g., committed in CI), allow the
            // build to proceed with the stale artifact rather than blocking CI entirely.
            let output_path = std::path::Path::new(&output);
            if !output_path.exists() {
                eprintln!("Fatal: {} does not exist. Cannot continue.", output);
                std::process::exit(1);
            } else {
                eprintln!("Warning: using existing {} — CSS may be stale.", output);
            }
        }
    }
}
