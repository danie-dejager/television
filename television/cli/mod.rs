use crate::{
    action::{Action, Actions},
    cable::{self, Cable},
    channels::prototypes::{ChannelPrototype, Template},
    cli::args::{Cli, Command},
    config::{
        DEFAULT_PREVIEW_SIZE, KeyBindings, get_config_dir, get_data_dir,
        merge_bindings,
        ui::{BorderType, Padding},
    },
    errors::cli_parsing_error_exit,
    event::Key,
    screen::layout::Orientation,
    utils::paths::expand_tilde,
};
use anyhow::{Result, anyhow};
use clap::CommandFactory;
use clap::error::ErrorKind;
use colored::Colorize;
use rustc_hash::FxHashMap;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use tracing::debug;

pub mod args;

/// # CLI Use Cases
///
/// The CLI interface supports two primary use cases:
///
/// ## 1. Channel-based mode (channel is specified)
/// When a channel is provided, the CLI operates in **override mode**:
/// - The channel provides the base configuration (source, preview, UI settings)
/// - All CLI flags act as **overrides** to the channel's defaults
/// - Most restrictions are enforced at the clap level using `conflicts_with`
/// - Templates and keybindings are validated after clap parsing
/// - More permissive - allows any combination of flags as they override channel defaults
///
/// ## 2. Ad-hoc mode (no channel specified)
/// When no channel is provided, the CLI creates an **ad-hoc channel**:
/// - Stricter validation rules apply for interdependent flags
/// - `--preview-*` flags require `--preview-command` to be set
/// - `--source-*` flags require `--source-command` to be set
/// - This ensures the ad-hoc channel has all necessary components to function
///
/// The validation logic in `post_process()` enforces these constraints for ad-hoc mode
/// while allowing full flexibility in channel-based mode.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct PostProcessedCli {
    // Channel and source configuration
    pub channel: Option<String>,
    pub source_command_override: Option<Template>,
    pub source_display_override: Option<Template>,
    pub source_output_override: Option<Template>,
    pub source_entry_delimiter: Option<char>,
    pub working_directory: Option<PathBuf>,
    pub autocomplete_prompt: Option<String>,
    pub ansi: bool,

    // Preview configuration
    pub preview_command_override: Option<Template>,
    pub preview_offset_override: Option<Template>,
    pub no_preview: bool,
    pub hide_preview: bool,
    pub show_preview: bool,
    pub preview_size: Option<u16>,
    pub preview_header: Option<String>,
    pub preview_footer: Option<String>,
    pub preview_border: Option<BorderType>,
    pub preview_padding: Option<Padding>,

    // Results panel configuration
    pub results_border: Option<BorderType>,
    pub results_padding: Option<Padding>,

    // Status bar configuration
    pub no_status_bar: bool,
    pub hide_status_bar: bool,
    pub show_status_bar: bool,

    // Remote configuration
    pub no_remote: bool,
    pub hide_remote: bool,
    pub show_remote: bool,

    // Help panel configuration
    pub no_help_panel: bool,
    pub hide_help_panel: bool,
    pub show_help_panel: bool,

    // Input configuration
    pub input: Option<String>,
    pub input_header: Option<String>,
    pub input_prompt: Option<String>,
    pub input_border: Option<BorderType>,
    pub input_padding: Option<Padding>,

    // UI and layout configuration
    pub layout: Option<Orientation>,
    pub ui_scale: Option<u16>,
    pub height: Option<u16>,
    pub width: Option<u16>,
    pub inline: bool,

    // Behavior and matching configuration
    pub exact: bool,
    pub select_1: bool,
    pub take_1: bool,
    pub take_1_fast: bool,
    pub keybindings: Option<KeyBindings>,

    // Performance configuration
    pub tick_rate: Option<f64>,
    pub watch_interval: Option<f64>,

    // History configuration
    pub global_history: bool,

    // Configuration sources
    pub config_file: Option<PathBuf>,
    pub cable_dir: Option<PathBuf>,

    // Command handling
    pub command: Option<Command>,
}

const DEFAULT_BORDER_TYPE: BorderType = BorderType::Rounded;

impl Default for PostProcessedCli {
    fn default() -> Self {
        Self {
            // Channel and source configuration
            channel: None,
            source_command_override: None,
            source_display_override: None,
            source_output_override: None,
            source_entry_delimiter: None,
            working_directory: None,
            autocomplete_prompt: None,
            ansi: false,

            // Preview configuration
            preview_command_override: None,
            preview_offset_override: None,
            no_preview: false,
            hide_preview: false,
            show_preview: false,
            preview_size: Some(DEFAULT_PREVIEW_SIZE),
            preview_header: None,
            preview_footer: None,
            preview_border: Some(DEFAULT_BORDER_TYPE),
            preview_padding: None,

            // Results panel configuration
            results_border: Some(DEFAULT_BORDER_TYPE),
            results_padding: None,

            // Status bar configuration
            no_status_bar: false,
            hide_status_bar: false,
            show_status_bar: false,

            // Remote configuration
            no_remote: false,
            hide_remote: false,
            show_remote: false,

            // Help panel configuration
            no_help_panel: false,
            hide_help_panel: false,
            show_help_panel: false,

            // Input configuration
            input: None,
            input_header: None,
            input_prompt: None,
            input_border: Some(DEFAULT_BORDER_TYPE),
            input_padding: None,

            // UI and layout configuration
            layout: None,
            ui_scale: None,
            height: None,
            width: None,
            inline: false,

            // Behavior and matching configuration
            exact: false,
            select_1: false,
            take_1: false,
            take_1_fast: false,
            keybindings: None,

            // Performance configuration
            tick_rate: None,
            watch_interval: None,

            // History configuration
            global_history: false,

            // Configuration sources
            config_file: None,
            cable_dir: None,

            // Command handling
            command: None,
        }
    }
}

/// Post-processes the raw CLI arguments into a structured format with validation.
///
/// This function handles the two main CLI use cases:
///
/// **Channel-based mode**: When `cli.channel` is provided, all flags are treated as
/// overrides to the channel's configuration. Validation is minimal since the channel
/// provides sensible defaults.
///
/// **Ad-hoc mode**: When no channel is specified, stricter validation ensures that
/// interdependent flags are used correctly:
/// - Preview flags (`--preview-offset`, `--preview-size`, etc.) require `--preview-command`
/// - Source flags (`--source-display`, `--source-output`) require `--source-command`
///
/// This prevents creating broken ad-hoc channels that reference non-existent commands.
pub fn post_process(cli: Cli, readable_stdin: bool) -> PostProcessedCli {
    // Parse literal keybindings passed through the CLI
    let mut keybindings = cli.keybindings.as_ref().map(|kb| {
        parse_keybindings_literal(kb, CLI_KEYBINDINGS_DELIMITER)
            .unwrap_or_else(|e| cli_parsing_error_exit(&e.to_string()))
    });

    // if `--expect` is used, parse and add to the keybindings
    if let Some(expect) = &cli.expect {
        let expect_bindings = parse_expect_bindings(expect)
            .unwrap_or_else(|e| cli_parsing_error_exit(&e.to_string()));
        keybindings = match keybindings {
            Some(kb) => {
                // Merge expect bindings into existing keybindings
                Some(merge_bindings(kb, &expect_bindings))
            }
            None => {
                // Initialize keybindings with expect bindings
                Some(expect_bindings)
            }
        }
    }

    // Parse preview overrides if provided
    let preview_command_override =
        cli.preview_command.as_ref().map(|preview_cmd| {
            Template::parse(preview_cmd).unwrap_or_else(|e| {
                cli_parsing_error_exit(&format!(
                    "Error parsing preview command: {e}"
                ))
            })
        });

    let preview_offset_override =
        cli.preview_offset.as_ref().map(|offset_str| {
            Template::parse(offset_str).unwrap_or_else(|e| {
                cli_parsing_error_exit(&format!(
                    "Error parsing preview offset: {e}"
                ))
            })
        });

    if cli.autocomplete_prompt.is_some() {
        if let Some(ch) = &cli.channel {
            if !Path::new(ch).exists() {
                let mut cmd = Cli::command();
                let arg1 = "'--autocomplete-prompt <STRING>'".yellow();
                let arg2 = "'[CHANNEL]'".yellow();
                let msg = format!(
                    "The argument {} cannot be used with {}",
                    arg1, arg2
                );
                cmd.error(ErrorKind::ArgumentConflict, msg).exit();
            }
        }
    }

    // Validate interdependent flags for ad-hoc mode (when no channel is specified)
    // This ensures ad-hoc channels have all necessary components to function properly
    validate_adhoc_mode_constraints(&cli, readable_stdin);

    // Validate width flag requires inline or height
    if cli.width.is_some() && !cli.inline && cli.height.is_none() {
        cli_parsing_error_exit(
            "--width can only be used in combination with --inline or --height",
        );
    }

    // Determine channel and working_directory
    let (channel, working_directory) = match &cli.channel {
        Some(c) if Path::new(c).exists() => {
            // If the channel is a path, use it as the working directory
            (None, Some(PathBuf::from(c)))
        }
        _ => (
            cli.channel.clone(),
            cli.working_directory.as_ref().map(PathBuf::from),
        ),
    };

    // Parse source overrides if any source fields are provided
    let source_command_override =
        cli.source_command.as_ref().map(|source_cmd| {
            Template::parse(source_cmd).unwrap_or_else(|e| {
                cli_parsing_error_exit(&format!(
                    "Error parsing source command: {e}"
                ))
            })
        });

    let source_display_override =
        cli.source_display.as_ref().map(|display_str| {
            Template::parse(display_str).unwrap_or_else(|e| {
                cli_parsing_error_exit(&format!(
                    "Error parsing source display: {e}"
                ))
            })
        });

    let source_output_override =
        cli.source_output.as_ref().map(|output_str| {
            Template::parse(output_str).unwrap_or_else(|e| {
                cli_parsing_error_exit(&format!(
                    "Error parsing source output: {e}"
                ))
            })
        });

    // Validate that the source entry delimiter is a single character
    let source_entry_delimiter =
        cli.source_entry_delimiter.as_ref().map(|delimiter| {
            parse_source_entry_delimiter(delimiter)
                .unwrap_or_else(|e| cli_parsing_error_exit(&e.to_string()))
        });

    // Determine layout
    let layout: Option<Orientation> = cli.layout.map(Orientation::from);

    // borders
    let input_border = cli.input_border.map(BorderType::from);
    let results_border = cli.results_border.map(BorderType::from);
    let preview_border = cli.preview_border.map(BorderType::from);

    // padding
    let input_padding = cli.input_padding.map(|p| {
        parse_padding_literal(&p, CLI_PADDING_DELIMITER)
            .unwrap_or_else(|e| cli_parsing_error_exit(&e.to_string()))
    });
    let results_padding = cli.results_padding.map(|p| {
        parse_padding_literal(&p, CLI_PADDING_DELIMITER)
            .unwrap_or_else(|e| cli_parsing_error_exit(&e.to_string()))
    });
    let preview_padding = cli.preview_padding.map(|p| {
        parse_padding_literal(&p, CLI_PADDING_DELIMITER)
            .unwrap_or_else(|e| cli_parsing_error_exit(&e.to_string()))
    });

    PostProcessedCli {
        // Channel and source configuration
        channel,
        source_command_override,
        source_display_override,
        source_output_override,
        source_entry_delimiter,
        working_directory,
        autocomplete_prompt: cli.autocomplete_prompt,
        ansi: cli.ansi,

        // Preview configuration
        preview_command_override,
        preview_offset_override,
        no_preview: cli.no_preview,
        hide_preview: cli.hide_preview,
        show_preview: cli.show_preview,
        preview_size: cli.preview_size,
        preview_header: cli.preview_header,
        preview_footer: cli.preview_footer,
        preview_border,
        preview_padding,

        // Results configuration
        results_border,
        results_padding,

        // Status bar configuration
        no_status_bar: cli.no_status_bar,
        hide_status_bar: cli.hide_status_bar,
        show_status_bar: cli.show_status_bar,

        // Remote configuration
        no_remote: cli.no_remote,
        hide_remote: cli.hide_remote,
        show_remote: cli.show_remote,

        // Help panel configuration
        no_help_panel: cli.no_help_panel,
        hide_help_panel: cli.hide_help_panel,
        show_help_panel: cli.show_help_panel,

        // Input configuration
        input: cli.input,
        input_header: cli.input_header,
        input_prompt: cli.input_prompt,
        input_border,
        input_padding,

        // UI and layout configuration
        layout,
        ui_scale: cli.ui_scale,
        height: cli.height,
        width: cli.width,
        inline: cli.inline,

        // Behavior and matching configuration
        exact: cli.exact,
        select_1: cli.select_1,
        take_1: cli.take_1,
        take_1_fast: cli.take_1_fast,
        keybindings,

        // Performance configuration
        tick_rate: cli.tick_rate,
        watch_interval: cli.watch,

        // History configuration
        global_history: cli.global_history,

        // Configuration sources
        config_file: cli.config_file.map(|p| expand_tilde(&p)),
        cable_dir: cli.cable_dir.map(|p| expand_tilde(&p)),

        // Command handling
        command: cli.command,
    }
}

/// Validates interdependent flags when operating in ad-hoc mode (no channel specified).
///
/// In ad-hoc mode, certain flags require their corresponding command to be specified:
/// - Source-related flags (`--source-display`, `--source-output`) require `--source-command`
/// - Preview-related flags (`--preview-offset`, `--preview-size`, etc.) require `--preview-command`
///
/// This validation ensures that ad-hoc channels have all necessary components to function.
/// When a channel is specified, these validations are skipped as the channel provides defaults.
fn validate_adhoc_mode_constraints(cli: &Cli, readable_stdin: bool) {
    // Skip validation if a channel is specified (channel-based mode)
    if cli.channel.is_some() {
        return;
    }

    // Validate source-related flags in ad-hoc mode
    if cli.source_command.is_none() && !readable_stdin {
        let source_flags = [
            ("--source-display", cli.source_display.is_some()),
            ("--source-output", cli.source_output.is_some()),
            ("--preview-command", cli.preview_command.is_some()),
        ];

        for (flag_name, is_set) in source_flags {
            if is_set {
                cli_parsing_error_exit(&format!(
                    "{} requires a source command when no channel is specified. \
                     Either specify a channel (which may have its own source command) or provide --source-command.",
                    flag_name
                ));
            }
        }
    }

    // Validate preview-related flags in ad-hoc mode
    if cli.preview_command.is_none() {
        let preview_flags = [
            ("--preview-offset", cli.preview_offset.is_some()),
            ("--preview-size", cli.preview_size.is_some()),
            ("--preview-header", cli.preview_header.is_some()),
            ("--preview-footer", cli.preview_footer.is_some()),
        ];

        for (flag_name, is_set) in preview_flags {
            if is_set {
                cli_parsing_error_exit(&format!(
                    "{} requires a preview command when no channel is specified. \
                     Either specify a channel (which may have its own preview command) or provide --preview-command.",
                    flag_name
                ));
            }
        }
    }
}

const CLI_KEYBINDINGS_DELIMITER: char = ';';
const CLI_PADDING_DELIMITER: char = ';';

/// Parse a keybindings literal into a `KeyBindings` struct.
///
/// The formalism used is the same as the one used in the configuration file:
/// ```ignore
///     quit="esc";select_next_entry=["down","ctrl-j"]
/// ```
/// Parsing it globally consists of splitting by the delimiter, reconstructing toml key-value pairs
/// and parsing that using logic already implemented in the configuration module.
fn parse_keybindings_literal(
    cli_keybindings: &str,
    delimiter: char,
) -> Result<KeyBindings> {
    let toml_definition = cli_keybindings
        .split(delimiter)
        .fold(String::new(), |acc, kb| acc + kb + "\n");

    toml::from_str(&toml_definition).map_err(|e| anyhow!(e))
}

/// Parses the CLI padding values into `config::ui::Padding`
fn parse_padding_literal(
    cli_padding: &str,
    delimiter: char,
) -> Result<Padding> {
    let toml_definition = cli_padding
        .split(delimiter)
        .fold(String::new(), |acc, p| acc + p + "\n");
    toml::from_str(&toml_definition).map_err(|e| anyhow!(e))
}

pub fn list_channels<P>(cable_dir: P)
where
    P: AsRef<Path>,
{
    let channels = cable::load_cable(cable_dir).unwrap_or_default();
    for c in channels.keys() {
        println!("{c}");
    }
}

pub fn parse_source_entry_delimiter(delimiter: &str) -> Result<char> {
    if delimiter.is_empty() {
        return Err(anyhow!("Source entry delimiter cannot be empty"));
    }
    if let Some(b) = delimiter.strip_prefix(r"\") {
        match b {
            "n" => return Ok('\n'),
            "t" => return Ok('\t'),
            "r" => return Ok('\r'),
            "0" => return Ok('\0'),
            _ => {
                return Err(anyhow!(
                    "Invalid escape sequence for source entry delimiter: '{}'",
                    b
                ));
            }
        }
    }
    if delimiter.len() != 1 {
        return Err(anyhow!(
            "Source entry delimiter must be a single character, got '{}'",
            delimiter
        ));
    }
    Ok(delimiter.chars().next().unwrap())
}

/// Backtrack from the end of the prompt and try to match each word to a known command
/// if a match is found, return the corresponding channel
/// if no match is found, throw an error
///
/// ## Example:
/// ```ignore
/// use television::channels::CliTvChannel;
/// use television::cli::ParsedCliChannel;
/// use television::cli::guess_channel_from_prompt;
///
/// let prompt = "ls -l";
/// let command_mapping = hashmap! {
///     "ls".to_string() => "files".to_string(),
///     "cat".to_string() => "files".to_string(),
/// };
/// let channel = guess_channel_from_prompt(prompt, &command_mapping).unwrap();
///
/// assert_eq!(channel, ParsedCliChannel::Builtin(CliTvChannel::Files));
/// ```
/// NOTE: this is a very naive implementation and needs to be improved
/// - it should be able to handle prompts containing multiple commands
///   e.g. "ls -l && cat <CTRL+T>"
/// - it should be able to handle commands within delimiters (quotes, brackets, etc.)
pub fn guess_channel_from_prompt(
    prompt: &str,
    command_mapping: &FxHashMap<String, String>,
    fallback_channel: &str,
    cable: &Cable,
) -> ChannelPrototype {
    debug!("Guessing channel from prompt: {}", prompt);
    // git checkout -qf
    // --- -------- --- <---------
    let fallback = cable.get_channel(fallback_channel);
    if prompt.trim().is_empty() {
        return fallback;
    }
    let rev_prompt_words = prompt.split_whitespace().rev();
    let mut stack = Vec::new();
    // for each patern
    for (pattern, channel) in command_mapping {
        if pattern.trim().is_empty() {
            continue;
        }
        // push every word of the pattern onto the stack
        stack.extend(pattern.split_whitespace());
        for word in rev_prompt_words.clone() {
            // if the stack is empty, we have a match
            if stack.is_empty() {
                return cable.get_channel(channel);
            }
            // if the word matches the top of the stack, pop it
            if stack.last() == Some(&word) {
                stack.pop();
            }
        }
        // if the stack is empty, we have a match
        if stack.is_empty() {
            return cable.get_channel(channel);
        }
        // reset the stack
        stack.clear();
    }

    debug!("No match found, falling back to default channel");
    fallback
}

/// Parses the `expect` keybindings from the CLI into a `KeyBindings` struct that can be
/// merged with other keybindings.
///
/// The `expect` keybindings are expected to be in the format:
/// ```ignore
/// "ctrl-q;esc"
/// ```
/// And will produce a `KeyBindings` struct with the following entries:
/// ```ignore
/// {
///    Key::Ctrl('q') => Actions::single(Action::Expect(Key::Ctrl('q'))),
///    Key::Esc => Actions::single(Action::Expect(Key::Esc)),
/// }
/// ```
fn parse_expect_bindings(expect: &str) -> Result<KeyBindings> {
    let mut bindings = KeyBindings::default();
    for s in expect.split(CLI_KEYBINDINGS_DELIMITER) {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        let key = Key::from_str(s).map_err(|e| {
            anyhow!("Invalid key in expect bindings: '{}'. Error: {}", s, e)
        })?;
        let action = Action::Expect(key);
        bindings.insert(key, Actions::single(action));
    }
    Ok(bindings)
}

const VERSION_MESSAGE: &str = env!("CARGO_PKG_VERSION");

pub fn version() -> String {
    let author = clap::crate_authors!();

    // let current_exe_path = PathBuf::from(clap::crate_name!()).display().to_string();
    let config_dir_path = get_config_dir().display().to_string();
    let data_dir_path = get_data_dir().display().to_string();

    format!(
        "\
{VERSION_MESSAGE}

           _______________
          |,----------.  |\\
          ||           |=| |
          ||          || | |
          ||       . _o| | |
          |`-----------' |/
           ~~~~~~~~~~~~~~~
  __      __         _     _
 / /____ / /__ _  __(_)__ (_)__  ___
/ __/ -_) / -_) |/ / (_-</ / _ \\/ _ \\
\\__/\\__/_/\\__/|___/_/___/_/\\___/_//_/

Authors: {author}

Config directory: {config_dir_path}
Data directory: {data_dir_path}"
    )
}

#[cfg(test)]
mod tests {
    use crate::{action::Action, event::Key};

    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_from_cli() {
        let cli = Cli {
            channel: Some("files".to_string()),
            preview_command: Some("bat -n --color=always {}".to_string()),
            working_directory: Some("/home/user".to_string()),
            ..Default::default()
        };

        let post_processed_cli = post_process(cli, false);

        assert_eq!(
            post_processed_cli.preview_command_override.unwrap().raw(),
            "bat -n --color=always {}".to_string(),
        );
        assert_eq!(post_processed_cli.tick_rate, None);
        assert_eq!(
            post_processed_cli.working_directory,
            Some(PathBuf::from("/home/user"))
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_from_cli_no_args() {
        let cli = Cli {
            channel: Some(".".to_string()),
            ..Default::default()
        };

        let post_processed_cli = post_process(cli, false);

        assert_eq!(
            post_processed_cli.working_directory,
            Some(PathBuf::from("."))
        );
        assert_eq!(post_processed_cli.command, None);
    }

    #[test]
    #[ignore = "expects binding toml structure"]
    fn test_custom_keybindings() {
        let cli = Cli {
            channel: Some("files".to_string()),
            preview_command: Some(":env_var:".to_string()),
            keybindings: Some(
                r#"esc="quit";down="select_next_entry";ctrl-j="select_next_entry""#
                    .to_string(),
            ),
            ..Default::default()
        };

        let post_processed_cli = post_process(cli, false);

        let mut expected = KeyBindings::default();
        expected.insert(Key::Esc, Action::Quit.into());
        expected.insert(Key::Down, Action::SelectNextEntry.into());
        expected.insert(Key::Ctrl('j'), Action::SelectNextEntry.into());

        assert_eq!(post_processed_cli.keybindings, Some(expected));
    }

    /// Returns a tuple containing a command mapping and a fallback channel.
    fn guess_channel_from_prompt_setup<'a>()
    -> (FxHashMap<String, String>, &'a str, Cable) {
        let mut command_mapping = FxHashMap::default();
        command_mapping.insert("vim".to_string(), "files".to_string());
        command_mapping.insert("export".to_string(), "env".to_string());

        (
            command_mapping,
            "env",
            Cable::from_prototypes(vec![
                ChannelPrototype::new("files", "fd -t f"),
                ChannelPrototype::new("env", "env"),
                ChannelPrototype::new("git", "git status"),
            ]),
        )
    }

    #[test]
    fn test_guess_channel_from_prompt_present() {
        let prompt = "vim -d file1";

        let (command_mapping, fallback, channels) =
            guess_channel_from_prompt_setup();

        let channel = guess_channel_from_prompt(
            prompt,
            &command_mapping,
            fallback,
            &channels,
        );

        assert_eq!(channel.metadata.name, "files");
    }

    #[test]
    fn test_guess_channel_from_prompt_fallback() {
        let prompt = "git checkout ";

        let (command_mapping, fallback, channels) =
            guess_channel_from_prompt_setup();

        let channel = guess_channel_from_prompt(
            prompt,
            &command_mapping,
            fallback,
            &channels,
        );

        assert_eq!(channel.metadata.name, fallback);
    }

    #[test]
    fn test_guess_channel_from_prompt_empty() {
        let prompt = "";

        let (command_mapping, fallback, channels) =
            guess_channel_from_prompt_setup();

        let channel = guess_channel_from_prompt(
            prompt,
            &command_mapping,
            fallback,
            &channels,
        );

        assert_eq!(channel.metadata.name, fallback);
    }

    #[test]
    /// We should be able to use a custom preview and custom headers/footers with stdin
    fn test_validate_adhoc_mode_constraints_stdin() {
        let cli = Cli {
            source_display: Some("display".to_string()),
            source_output: Some("output".to_string()),
            preview_command: Some("preview".to_string()),
            preview_offset: Some("offset".to_string()),
            preview_size: Some(10),
            preview_header: Some("header".to_string()),
            preview_footer: Some("footer".to_string()),
            ..Default::default()
        };

        validate_adhoc_mode_constraints(&cli, true);
    }

    #[test]
    fn test_parse_expect_keybindings() {
        let expect = "ctrl-q;esc";
        let bindings = parse_expect_bindings(expect).unwrap();

        let mut expected = KeyBindings::default();
        expected.insert(
            Key::Ctrl('q'),
            Actions::single(Action::Expect(Key::Ctrl('q'))),
        );
        expected.insert(Key::Esc, Actions::single(Action::Expect(Key::Esc)));

        assert_eq!(bindings, expected);
    }
}
