//! The command line. `--help` and `--version` answer and exit before any
//! window opens, as other programs do; anything else the app doesn't know
//! is refused rather than ignored, so a mistyped option doesn't quietly start
//! the app.

use std::ffi::OsString;

/// The name the app is started by.
const PROGRAM: &str = "link2linux-desktop";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Start the app.
    Run,
    /// Print this to standard output and exit successfully.
    Print(String),
    /// Print this to standard error and exit with a usage error.
    Refuse(String),
}

/// Reads the arguments after the program's name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Command {
    let Some(first) = args.into_iter().next() else {
        return Command::Run;
    };
    match first.to_str() {
        Some("-h" | "--help") => Command::Print(help()),
        Some("-V" | "--version") => {
            Command::Print(format!("link2linux {}\n", env!("CARGO_PKG_VERSION")))
        }
        _ => {
            let shown = first.to_string_lossy();
            let what = if shown.starts_with('-') {
                format!("unrecognized option '{shown}'")
            } else {
                format!("unexpected argument '{shown}'")
            };
            Command::Refuse(format!(
                "{PROGRAM}: {what}\nTry '{PROGRAM} --help' for more information.\n"
            ))
        }
    }
}

fn help() -> String {
    format!(
        "Usage: {PROGRAM} [OPTION]\n\
         Start the link2linux desktop app, which connects your Android phone to\n\
         this computer.\n\
         \n\
         Options:\n\
         \x20 -h, --help     show this help and exit\n\
         \x20 -V, --version  show the version and exit\n\
         \n\
         Website: <https://codesensei-tushar.github.io/link2linux/>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Command {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_start_the_app() {
        assert_eq!(parse_strs(&[]), Command::Run);
    }

    #[test]
    fn the_version_names_the_app_and_its_version() {
        let expected = Command::Print(format!("link2linux {}\n", env!("CARGO_PKG_VERSION")));
        assert_eq!(parse_strs(&["--version"]), expected);
        assert_eq!(parse_strs(&["-V"]), expected);
    }

    #[test]
    fn help_lists_the_options() {
        for flag in ["--help", "-h"] {
            let Command::Print(text) = parse_strs(&[flag]) else {
                panic!("{flag} printed nothing");
            };
            assert!(
                text.starts_with("Usage: link2linux-desktop [OPTION]\n"),
                "{text}"
            );
            assert!(
                text.contains("  -V, --version  show the version and exit\n"),
                "{text}"
            );
        }
    }

    #[test]
    fn unknown_arguments_are_refused_with_a_pointer_to_help() {
        assert_eq!(
            parse_strs(&["--verison"]),
            Command::Refuse(
                "link2linux-desktop: unrecognized option '--verison'\n\
                 Try 'link2linux-desktop --help' for more information.\n"
                    .into()
            )
        );
        let Command::Refuse(text) = parse_strs(&["photo.jpg"]) else {
            panic!("a file name started the app");
        };
        assert!(
            text.starts_with("link2linux-desktop: unexpected argument 'photo.jpg'\n"),
            "{text}"
        );
    }
}
