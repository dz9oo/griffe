//! Choix du mode avant tout coffre, toute fenêtre et tout runtime.
//! Le nom d'appel et le premier argument décident. Aucune variable d'environnement.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Message fixe. Il ne reprend aucun argument : une commande de passphrase peut
/// contenir un secret.
pub(crate) const REFUSE_MESSAGE: &str = "\
Griffe n'ouvre pas la fenêtre avec ces arguments.
La fenêtre : griffe desktop, ou la commande griffe-desktop.
L'agent : griffe mcp.
La ligne de commande : griffe --help.
";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    /// argv0 est déjà `griffe-desktop`, sans autre argument.
    Desktop,
    /// Le jeton `desktop` doit quitter l'argv avant GTK, par un nouvel exec.
    DesktopToken,
    Mcp,
    Cli,
    Refuse,
}

pub(crate) fn launch(argv0: &OsStr, args: &[OsString]) -> Launch {
    let Some(argv0) = argv0.to_str() else {
        return Launch::Refuse;
    };
    let Some(base) = Path::new(argv0).file_name().and_then(|name| name.to_str()) else {
        return Launch::Refuse;
    };
    if base.is_empty() {
        return Launch::Refuse;
    }
    let mut user = Vec::with_capacity(args.len());
    for arg in args {
        let Some(text) = arg.to_str() else {
            return Launch::Refuse;
        };
        user.push(text);
    }

    match base {
        "griffe-desktop" => {
            if user.is_empty() {
                Launch::Desktop
            } else {
                Launch::Refuse
            }
        }
        "griffe-mcp" => {
            if user.is_empty() {
                Launch::Mcp
            } else {
                Launch::Refuse
            }
        }
        _ => match user.as_slice() {
            ["desktop"] => Launch::DesktopToken,
            ["mcp"] => Launch::Mcp,
            ["desktop", ..] | ["mcp", ..] => Launch::Refuse,
            _ => Launch::Cli,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStringExt;

    use super::{Launch, REFUSE_MESSAGE, launch};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().copied().map(OsString::from).collect()
    }

    #[test]
    fn the_name_and_the_first_argument_choose_the_mode() {
        let cases = [
            ("/usr/bin/griffe", &[][..], Launch::Cli),
            ("griffe", &[][..], Launch::Cli),
            ("/usr/bin/griffe", &["--help"][..], Launch::Cli),
            ("griffe", &["--version"][..], Launch::Cli),
            ("griffe", &["help"][..], Launch::Cli),
            ("griffe", &["--json", "invoice", "list"][..], Launch::Cli),
            (
                "griffe",
                &["--db", "/tmp/coffre.db", "invoice", "list"][..],
                Launch::Cli,
            ),
            (
                "griffe",
                &["--db", "/tmp/coffre.db", "desktop"][..],
                Launch::Cli,
            ),
            ("griffe", &["vault", "status"][..], Launch::Cli),
            ("/usr/bin/griffe", &["desktop"][..], Launch::DesktopToken),
            ("griffe", &["mcp"][..], Launch::Mcp),
            ("/usr/bin/griffe-desktop", &[][..], Launch::Desktop),
            ("/usr/lib/griffe/griffe-desktop", &[][..], Launch::Desktop),
            ("griffe-desktop", &[][..], Launch::Desktop),
            ("/usr/bin/griffe-mcp", &[][..], Launch::Mcp),
            ("griffe-mcp", &[][..], Launch::Mcp),
            ("Griffe_0.3.18_amd64.AppImage", &[][..], Launch::Cli),
            ("Desktop", &[][..], Launch::Cli),
            ("griffe", &["Desktop"][..], Launch::Cli),
            ("griffe", &["MCP"][..], Launch::Cli),
            ("griffe-desktop-old", &[][..], Launch::Cli),
            ("griffe-desktop.exe", &[][..], Launch::Cli),
            ("not-griffe", &[][..], Launch::Cli),
        ];
        for (argv0, user, want) in cases {
            assert_eq!(launch(OsStr::new(argv0), &args(user)), want, "{argv0}");
        }
    }

    #[test]
    fn a_mode_name_with_extra_arguments_is_refused() {
        let cases = [
            ("/usr/bin/griffe-desktop", &["mcp"][..]),
            ("griffe-desktop", &["--help"][..]),
            ("griffe-desktop", &["desktop"][..]),
            ("/usr/bin/griffe-mcp", &["desktop"][..]),
            ("griffe-mcp", &["--help"][..]),
            ("griffe", &["desktop", "--db", "/tmp/x"][..]),
            ("griffe", &["mcp", "--help"][..]),
            ("griffe", &["mcp", "--passphrase-file", "/tmp/secret"][..]),
        ];
        for (argv0, user) in cases {
            assert_eq!(
                launch(OsStr::new(argv0), &args(user)),
                Launch::Refuse,
                "{argv0}"
            );
        }
    }

    #[test]
    fn a_name_that_is_not_utf8_is_refused() {
        let bad = OsString::from_vec(vec![0xff]);
        assert_eq!(launch(&bad, &[]), Launch::Refuse);
        assert_eq!(
            launch(OsStr::new("griffe"), std::slice::from_ref(&bad)),
            Launch::Refuse
        );
        assert_eq!(launch(OsStr::new(""), &[]), Launch::Refuse);
    }

    #[test]
    fn the_refusal_does_not_repeat_the_command_line() {
        assert!(!REFUSE_MESSAGE.contains('{'));
        assert!(REFUSE_MESSAGE.contains("griffe desktop"));
        assert!(REFUSE_MESSAGE.contains("griffe-desktop"));
        assert!(REFUSE_MESSAGE.contains("griffe mcp"));
        assert!(REFUSE_MESSAGE.contains("griffe --help"));
        assert!(!REFUSE_MESSAGE.contains("passphrase"));
    }
}
