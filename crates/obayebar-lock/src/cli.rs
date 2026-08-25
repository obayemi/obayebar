//! Argument parsing for the lock screen.

use std::path::PathBuf;

pub const USAGE: &str = "\
obayebar-lock [OPTIONS]

  -c, --config <PATH>     Base hyprlock config to extend
      --state <PATH>      Wallpaper state file to read
      --no-wallpaper      Lock with the base config unchanged
      --print             Print the generated config and exit, without locking
      --check             Validate and exit non-zero on any problem
      --blur <P>x<S>      Blur passes and size, e.g. 2x5
  -g, --grace <SECS>      Seconds before a password is required
      --detach            Do not wait for hyprlock to exit
      --no-scope          Do not wrap hyprlock in its own systemd scope
  -h, --help              Print this help
  -V, --version           Print version";

// Five flags rather than an enum: they are genuinely independent switches,
// not modes, and every combination is meaningful.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub config: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub no_wallpaper: bool,
    pub print: bool,
    pub check: bool,
    pub blur: Option<(u32, u32)>,
    pub grace: Option<u32>,
    pub detach: bool,
    pub no_scope: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Usage(String),
    Exit(String),
}

/// Parse command-line arguments.
///
/// # Errors
///
/// Returns [`Error::Usage`] for an unknown flag or a bad value, and
/// [`Error::Exit`] for `--help` / `--version`.
pub fn parse<I: Iterator<Item = String>>(args: I) -> Result<Args, Error> {
    let mut out = Args::default();
    let mut iter = args;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(Error::Exit(USAGE.to_string())),
            "-V" | "--version" => {
                return Err(Error::Exit(format!(
                    "obayebar-lock {}",
                    env!("CARGO_PKG_VERSION")
                )))
            }
            "--no-wallpaper" => out.no_wallpaper = true,
            "--print" => out.print = true,
            "--check" => out.check = true,
            "--detach" => out.detach = true,
            "--no-scope" => out.no_scope = true,
            "-c" | "--config" => out.config = Some(PathBuf::from(value(&mut iter, &arg)?)),
            "--state" => out.state = Some(PathBuf::from(value(&mut iter, &arg)?)),
            "--blur" => out.blur = Some(blur(&value(&mut iter, &arg)?)?),
            "-g" | "--grace" => out.grace = Some(number(&value(&mut iter, &arg)?, &arg)?),
            other => {
                if let Some(rest) = other.strip_prefix("--config=") {
                    out.config = Some(PathBuf::from(rest));
                } else if let Some(rest) = other.strip_prefix("--state=") {
                    out.state = Some(PathBuf::from(rest));
                } else if let Some(rest) = other.strip_prefix("--blur=") {
                    out.blur = Some(blur(rest)?);
                } else if let Some(rest) = other.strip_prefix("--grace=") {
                    out.grace = Some(number(rest, "--grace")?);
                } else {
                    return Err(Error::Usage(format!("unknown argument '{other}'")));
                }
            }
        }
    }

    Ok(out)
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

fn number(raw: &str, flag: &str) -> Result<u32, Error> {
    raw.trim()
        .parse()
        .map_err(|_| Error::Usage(format!("{flag} wants a whole number, got {raw:?}")))
}

/// Parse `PASSESxSIZE`, e.g. `2x5`.
fn blur(raw: &str) -> Result<(u32, u32), Error> {
    let (passes, size) = raw
        .split_once('x')
        .ok_or_else(|| Error::Usage(format!("--blur wants PASSESxSIZE, got {raw:?}")))?;
    Ok((number(passes, "--blur")?, number(size, "--blur")?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Args, Error> {
        parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn no_arguments_locks_with_defaults() {
        assert_eq!(parse_args(&[]).unwrap(), Args::default());
    }

    #[test]
    fn separate_and_joined_values_both_work() {
        let a = parse_args(&["--config", "/etc/h.conf"]).unwrap();
        let b = parse_args(&["--config=/etc/h.conf"]).unwrap();
        assert_eq!(a.config, Some(PathBuf::from("/etc/h.conf")));
        assert_eq!(a, b);
    }

    #[test]
    fn each_value_flag_lands_in_its_own_field() {
        let args = parse_args(&["--state", "/s.json", "--config", "/c.conf", "-g", "5"]).unwrap();
        assert_eq!(args.state, Some(PathBuf::from("/s.json")));
        assert_eq!(args.config, Some(PathBuf::from("/c.conf")));
        assert_eq!(args.grace, Some(5));
    }

    #[test]
    fn blur_parses_as_passes_by_size() {
        assert_eq!(parse_args(&["--blur", "2x5"]).unwrap().blur, Some((2, 5)));
        assert_eq!(parse_args(&["--blur=0x0"]).unwrap().blur, Some((0, 0)));
    }

    #[test]
    fn a_malformed_blur_is_rejected() {
        for bad in ["2", "x", "2x", "ax5", "2x5x7"] {
            assert!(
                parse_args(&["--blur", bad]).is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn grace_wants_a_number() {
        assert_eq!(parse_args(&["--grace", "0"]).unwrap().grace, Some(0));
        assert!(parse_args(&["--grace", "soon"]).is_err());
    }

    #[test]
    fn boolean_flags_set_their_fields() {
        let args = parse_args(&[
            "--print",
            "--check",
            "--no-wallpaper",
            "--detach",
            "--no-scope",
        ])
        .unwrap();
        assert!(args.print && args.check && args.no_wallpaper && args.detach && args.no_scope);
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert!(parse_args(&["--wat"]).is_err());
        assert!(parse_args(&["positional"]).is_err());
    }

    #[test]
    fn a_missing_value_names_the_flag() {
        let Err(Error::Usage(message)) = parse_args(&["--config"]) else {
            panic!("expected a usage error");
        };
        assert!(message.contains("--config"), "got {message:?}");
    }

    #[test]
    fn help_and_version_exit_zero() {
        assert!(matches!(parse_args(&["-h"]), Err(Error::Exit(_))));
        assert!(matches!(parse_args(&["-V"]), Err(Error::Exit(_))));
    }
}
