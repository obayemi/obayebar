//! Hand-rolled argument parsing, matching the bar's.
//!
//! Split from `main` and taking an iterator rather than reading `env::args`
//! directly so the precedence and the error messages are testable — the bar's
//! parser is not, and it has a latent bug where every value-taking flag lands
//! in the same field.

use std::path::PathBuf;

use crate::control::Command;

pub const USAGE: &str = "\
obayebar-wallpaper [OPTIONS]

  -d, --directory <DIR>   Directory to pick wallpapers from
  -i, --interval <SPEC>   Rotation interval: 45s, 30m, 2h, 1d, or off
      --once              Assign once, write the state file, and exit
      --next              Ask the running daemon to rotate now
      --reload            Ask it to re-scan the wallpaper directory
  -h, --help              Print this help
  -V, --version           Print version";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub directory: Option<PathBuf>,
    /// Left as the raw string so an invalid spec is reported against the flag
    /// the user actually typed, with the same parser the config file uses.
    pub interval: Option<String>,
    pub once: bool,
    /// Set when this invocation is a client talking to a running daemon.
    pub command: Option<Command>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Bad usage: print to stderr, exit non-zero.
    Usage(String),
    /// `--help` / `--version`: print to stdout, exit zero.
    Exit(String),
}

/// Parse command-line arguments.
///
/// # Errors
///
/// Returns [`Error::Usage`] for an unknown flag, a missing value, or two
/// commands at once; [`Error::Exit`] for `--help` and `--version`.
pub fn parse<I: Iterator<Item = String>>(args: I) -> Result<Args, Error> {
    let mut out = Args::default();
    let mut iter = args.peekable();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(Error::Exit(USAGE.to_string())),
            "-V" | "--version" => {
                return Err(Error::Exit(format!(
                    "obayebar-wallpaper {}",
                    env!("CARGO_PKG_VERSION")
                )))
            }
            "--once" => out.once = true,
            "--next" => set_command(&mut out, Command::Next)?,
            "--reload" => set_command(&mut out, Command::Reload)?,
            "-d" | "--directory" => {
                out.directory = Some(PathBuf::from(value(&mut iter, &arg)?));
            }
            "-i" | "--interval" => {
                out.interval = Some(value(&mut iter, &arg)?);
            }
            other => {
                if let Some(rest) = other.strip_prefix("--directory=") {
                    out.directory = Some(PathBuf::from(rest));
                } else if let Some(rest) = other.strip_prefix("--interval=") {
                    out.interval = Some(rest.to_string());
                } else {
                    return Err(Error::Usage(format!("unknown argument '{other}'")));
                }
            }
        }
    }

    if out.command.is_some() && (out.once || out.directory.is_some() || out.interval.is_some()) {
        return Err(Error::Usage(
            "--next and --reload talk to a running daemon and take no other options".to_string(),
        ));
    }

    Ok(out)
}

fn set_command(out: &mut Args, command: Command) -> Result<(), Error> {
    if out.command.is_some_and(|existing| existing != command) {
        return Err(Error::Usage(
            "pass only one of --next and --reload".to_string(),
        ));
    }
    out.command = Some(command);
    Ok(())
}

fn value<I: Iterator<Item = String>>(iter: &mut I, flag: &str) -> Result<String, Error> {
    let raw = iter
        .next()
        .ok_or_else(|| Error::Usage(format!("{flag} requires a value")))?;
    if raw.trim().is_empty() {
        return Err(Error::Usage(format!("{flag} value cannot be empty")));
    }
    Ok(raw)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Args, Error> {
        parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn no_arguments_is_the_default_daemon() {
        let args = parse_args(&[]).unwrap();
        assert_eq!(args, Args::default());
        assert!(args.command.is_none());
    }

    #[test]
    fn separate_and_joined_values_both_work() {
        let a = parse_args(&["--directory", "/pics"]).unwrap();
        let b = parse_args(&["--directory=/pics"]).unwrap();
        assert_eq!(a.directory, Some(PathBuf::from("/pics")));
        assert_eq!(a, b);
    }

    #[test]
    fn short_flags_work() {
        let args = parse_args(&["-d", "/pics", "-i", "45s"]).unwrap();
        assert_eq!(args.directory, Some(PathBuf::from("/pics")));
        assert_eq!(args.interval.as_deref(), Some("45s"));
    }

    #[test]
    fn each_value_flag_lands_in_its_own_field() {
        // The bar's parser funnels every value into one field; this guards
        // against repeating that here.
        let args = parse_args(&["--interval", "2h", "--directory", "/pics"]).unwrap();
        assert_eq!(args.interval.as_deref(), Some("2h"));
        assert_eq!(args.directory, Some(PathBuf::from("/pics")));
    }

    #[test]
    fn a_missing_or_empty_value_is_rejected() {
        assert!(matches!(parse_args(&["--directory"]), Err(Error::Usage(_))));
        assert!(matches!(parse_args(&["--interval"]), Err(Error::Usage(_))));
        assert!(matches!(parse_args(&["-d", "  "]), Err(Error::Usage(_))));
    }

    #[test]
    fn the_error_names_the_flag_the_user_typed() {
        let Err(Error::Usage(message)) = parse_args(&["--interval"]) else {
            panic!("expected a usage error");
        };
        assert!(message.contains("--interval"), "got {message:?}");
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert!(matches!(parse_args(&["--wat"]), Err(Error::Usage(_))));
        assert!(matches!(parse_args(&["positional"]), Err(Error::Usage(_))));
    }

    #[test]
    fn help_and_version_exit_zero() {
        assert!(matches!(parse_args(&["--help"]), Err(Error::Exit(_))));
        assert!(matches!(parse_args(&["-h"]), Err(Error::Exit(_))));
        assert!(matches!(parse_args(&["-V"]), Err(Error::Exit(_))));
    }

    #[test]
    fn client_commands_parse() {
        assert_eq!(
            parse_args(&["--next"]).unwrap().command,
            Some(Command::Next)
        );
        assert_eq!(
            parse_args(&["--reload"]).unwrap().command,
            Some(Command::Reload)
        );
    }

    #[test]
    fn conflicting_commands_are_rejected() {
        assert!(matches!(
            parse_args(&["--next", "--reload"]),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn a_repeated_command_is_harmless() {
        assert_eq!(
            parse_args(&["--next", "--next"]).unwrap().command,
            Some(Command::Next)
        );
    }

    #[test]
    fn client_commands_refuse_daemon_options() {
        // --next dials a running daemon, so a directory here would silently do
        // nothing; saying so beats ignoring it.
        assert!(matches!(
            parse_args(&["--next", "--directory", "/pics"]),
            Err(Error::Usage(_))
        ));
        assert!(matches!(
            parse_args(&["--next", "--once"]),
            Err(Error::Usage(_))
        ));
    }
}
