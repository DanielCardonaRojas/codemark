use clap::Parser;
use codemark_cli::cli::{Cli, external, handlers, templates};
use codemark_core::error::exit_with_error;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // When built with `bundled-tui` (the default), `codemark tui` is served by
    // the `codemark-tui` library linked into this binary rather than a separate
    // executable — so single-binary installs (mise/script/PowerShell) ship the
    // dashboard for free. Run it in-process before any CLI initialization.
    #[cfg(feature = "bundled-tui")]
    if external::is_bundled_tui(&cli.command) {
        match codemark_tui::run().await {
            Ok(Some(code)) => std::process::exit(code),
            Ok(None) => return,
            Err(err) => {
                eprintln!("codemark: tui: {err}");
                std::process::exit(1);
            }
        }
    }

    // External/plugin subcommands (e.g. `codemark tui` -> `codemark-tui`) are
    // dispatched before any expensive initialization so the backing binary
    // launches immediately. On Unix this `exec()`s and never returns; otherwise
    // it returns only when the executable could not be launched. With
    // `bundled-tui` off, `tui` falls through here and runs a standalone binary.
    if let Some(err) = external::try_dispatch(&cli.command) {
        eprintln!("{}", err.message);
        std::process::exit(err.code);
    }

    // Write the shipped templates, refreshing any that are out of date.
    //
    // A template on disk always wins over the compiled-in default, so a copy
    // written by an older codemark would otherwise shadow it forever — a
    // release that adds a section renders nothing new for existing users.
    // Untouched copies are refreshed silently; edited ones are reported here,
    // since overwriting someone's customization is worse than an old rendering.
    for stale in templates::ensure_default_template_exists() {
        eprintln!(
            "codemark: {} is customized and predates this release, so sections added \
             since will not render.\n  Delete {} to take the current default.",
            stale.name,
            stale.path.display()
        );
    }

    if let Err(err) = handlers::dispatch(cli).await {
        exit_with_error(&err);
    }
}
