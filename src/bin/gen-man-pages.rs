use std::fs::create_dir_all;
use std::io::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let out_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let man_dir = out_dir.join("man");
    create_dir_all(&man_dir)?;

    let cmd = codemark::cli::build_cli();

    // Generate main man page
    let man = clap_mangen::Man::new(cmd.clone());
    let mut buffer: Vec<u8> = Default::default();
    man.render(&mut buffer)?;
    std::fs::write(man_dir.join("codemark.1"), buffer)?;

    // Generate subcommand man pages
    generate_for_subcommands(&cmd, &man_dir, &["codemark"])?;

    println!("Man pages generated in: {}", man_dir.display());
    println!("Install with: sudo install -Dm644 man/*.1 -t /usr/local/share/man/man1/");

    Ok(())
}

fn generate_for_subcommands(
    cmd: &clap::Command,
    man_dir: &PathBuf,
    parent_path: &[&str],
) -> Result<()> {
    for subcommand in cmd.get_subcommands() {
        let name = subcommand.get_name();

        // Skip completions subcommand
        if name == "completions" {
            continue;
        }

        let mut new_path = parent_path.to_vec();
        new_path.push(name);

        let man_page_name = new_path.join("-");
        let man_path = man_dir.join(format!("{}.1", man_page_name));

        let man = clap_mangen::Man::new(subcommand.clone());
        let mut buffer: Vec<u8> = Default::default();
        man.render(&mut buffer)?;
        std::fs::write(man_path, buffer)?;

        println!("  Generated: {}.1", man_page_name);

        // Recursively generate for nested subcommands (collection subcommands)
        generate_for_subcommands(subcommand, man_dir, &new_path)?;
    }
    Ok(())
}
