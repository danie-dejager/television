use std::env;
use std::io::{stdout, BufWriter, IsTerminal, Write};
use std::path::Path;
use std::process::exit;

use anyhow::Result;
use clap::Parser;
use television::utils::clipboard::CLIPBOARD;
use tracing::{debug, error, info};

use television::app::App;
use television::channels::{
    entry::PreviewType, stdin::Channel as StdinChannel, TelevisionChannel,
};
use television::cli::{
    guess_channel_from_prompt, list_channels, Cli, ParsedCliChannel,
    PostProcessedCli,
};

use television::config::{Config, ConfigEnv};
use television::utils::shell::render_autocomplete_script_template;
use television::utils::{
    shell::{completion_script, Shell},
    stdin::is_readable_stdin,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    television::errors::init()?;
    television::logging::init()?;

    let args: PostProcessedCli = Cli::parse().into();
    debug!("{:?}", args);

    let mut config = Config::new(&ConfigEnv::init()?)?;

    if let Some(command) = args.command {
        match command {
            television::cli::Command::ListChannels => {
                list_channels();
                exit(0);
            }
            television::cli::Command::InitShell { shell } => {
                let target_shell = Shell::from(shell);
                // the completion scripts for the various shells are templated
                // so that it's possible to override the keybindings triggering
                // shell autocomplete and command history in tv
                let script = render_autocomplete_script_template(
                    target_shell,
                    completion_script(target_shell)?,
                    &config.shell_integration,
                )?;
                println!("{script}");
                exit(0);
            }
        }
    }

    config.config.tick_rate =
        args.tick_rate.unwrap_or(config.config.tick_rate);
    if args.no_preview {
        config.ui.show_preview_panel = false;
    }

    if let Some(working_directory) = args.working_directory {
        let path = Path::new(&working_directory);
        if !path.exists() {
            error!(
                "Working directory \"{}\" does not exist",
                &working_directory
            );
            println!(
                "Error: Working directory \"{}\" does not exist",
                &working_directory
            );
            exit(1);
        }
        env::set_current_dir(path)?;
    }

    CLIPBOARD.with(<_>::default);

    match App::new(
        {
            if is_readable_stdin() {
                debug!("Using stdin channel");
                TelevisionChannel::Stdin(StdinChannel::new(
                    args.preview_command.map(PreviewType::Command),
                ))
            } else if let Some(prompt) = args.autocomplete_prompt {
                let channel = guess_channel_from_prompt(
                    &prompt,
                    &config.shell_integration.commands,
                )?;
                debug!("Using guessed channel: {:?}", channel);
                match channel {
                    ParsedCliChannel::Builtin(c) => c.to_channel(),
                    ParsedCliChannel::Cable(c) => {
                        TelevisionChannel::Cable(c.into())
                    }
                }
            } else {
                debug!("Using {:?} channel", args.channel);
                match args.channel {
                    ParsedCliChannel::Builtin(c) => c.to_channel(),
                    ParsedCliChannel::Cable(c) => {
                        TelevisionChannel::Cable(c.into())
                    }
                }
            }
        },
        config,
        &args.passthrough_keybindings,
        args.input,
    ) {
        Ok(mut app) => {
            stdout().flush()?;
            let output = app.run(stdout().is_terminal()).await?;
            info!("{:?}", output);
            // lock stdout
            let stdout_handle = stdout().lock();
            let mut bufwriter = BufWriter::new(stdout_handle);
            if let Some(passthrough) = output.passthrough {
                writeln!(bufwriter, "{passthrough}")?;
            }
            if let Some(entries) = output.selected_entries {
                for entry in &entries {
                    writeln!(bufwriter, "{}", entry.stdout_repr())?;
                }
            }
            bufwriter.flush()?;
            exit(0);
        }
        Err(err) => {
            println!("{err:?}");
            exit(1);
        }
    }
}
