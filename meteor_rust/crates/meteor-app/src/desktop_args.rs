use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct DesktopArgs {
    pub original_dir: Option<PathBuf>,
    pub check_resources: bool,
    pub bake_map_previews: bool,
    pub help: bool,
    pub bot_host: bool,
    pub bot_python: Option<PathBuf>,
    pub bot_script: Option<PathBuf>,
    pub bot_policy: Option<PathBuf>,
    pub bot_stage: String,
    pub bot_room: String,
    pub bot_name: String,
    pub bot_character: u8,
    pub bot_weapon: u8,
    pub bot_port: u16,
}
impl Default for DesktopArgs {
    fn default() -> Self {
        Self {
            original_dir: None,
            check_resources: false,
            bake_map_previews: false,
            help: false,
            bot_host: false,
            bot_python: None,
            bot_script: None,
            bot_policy: None,
            bot_stage: "sn06".into(),
            bot_room: "MRC BOT sn06".into(),
            bot_name: "MRC Bot".into(),
            bot_character: 0,
            bot_weapon: 5,
            bot_port: 5230,
        }
    }
}

impl DesktopArgs {
    pub fn resource_directory(&self) -> Option<PathBuf> {
        self.original_dir.clone().or_else(|| {
            (self.bot_host || cfg!(feature = "original-directory-default"))
                .then(|| PathBuf::from("."))
        })
    }

    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let text = arg.to_str().ok_or("argument name is not UTF-8")?.to_owned();
            if text == "-h" {
                options.help = true;
                continue;
            }
            let (name, inline) = match text.split_once('=') {
                Some((name, value)) if name.starts_with("--") => {
                    (name.to_string(), Some(OsString::from(value)))
                }
                _ => (text, None),
            };
            if !name.starts_with("--") {
                return Err(format!("Unknown argument: {name}"));
            }
            let mut value = |flag: bool| -> Result<Option<OsString>, String> {
                if flag {
                    return match inline {
                        Some(_) => Err(format!("{name} does not take a value")),
                        None => Ok(None),
                    };
                }
                match inline.clone() {
                    Some(value) if !value.is_empty() => Ok(Some(value)),
                    Some(_) => Err(format!("{name} requires a value")),
                    None => args
                        .next()
                        .filter(|value| {
                            !value.is_empty() && !value.to_string_lossy().starts_with("--")
                        })
                        .map(Some)
                        .ok_or_else(|| format!("{name} requires a value")),
                }
            };
            match name.as_str() {
                "--original-dir" => {
                    let path = PathBuf::from(value(false)?.unwrap());
                    if options.original_dir.replace(path).is_some() {
                        return Err("--original-dir may only be specified once".into());
                    }
                }
                "--bake-map-previews" => {
                    value(true)?;
                    options.bake_map_previews = true;
                }
                "--check-resources" => {
                    value(true)?;
                    options.check_resources = true;
                }
                "--help" => {
                    value(true)?;
                    options.help = true;
                }
                "--bot-host" => {
                    value(true)?;
                    options.bot_host = true;
                }
                "--bot-python" => options.bot_python = Some(PathBuf::from(value(false)?.unwrap())),
                "--bot-script" => options.bot_script = Some(PathBuf::from(value(false)?.unwrap())),
                "--bot-policy" => options.bot_policy = Some(PathBuf::from(value(false)?.unwrap())),
                "--bot-stage" => {
                    options.bot_stage = value(false)?.unwrap().to_string_lossy().into_owned()
                }
                "--bot-room" => {
                    options.bot_room = value(false)?.unwrap().to_string_lossy().into_owned()
                }
                "--bot-name" => {
                    options.bot_name = value(false)?.unwrap().to_string_lossy().into_owned()
                }
                "--bot-character" => {
                    options.bot_character = number("--bot-character", value(false)?.unwrap())?
                }
                "--bot-weapon" => {
                    options.bot_weapon = number("--bot-weapon", value(false)?.unwrap())?
                }
                "--bot-port" => options.bot_port = number("--bot-port", value(false)?.unwrap())?,
                _ => return Err(format!("Unknown argument: {name}")),
            }
        }
        Ok(options)
    }
}
fn number<T: std::str::FromStr>(name: &str, value: OsString) -> Result<T, String> {
    value
        .to_string_lossy()
        .parse()
        .map_err(|_| format!("{name} requires a number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<DesktopArgs, String> {
        DesktopArgs::parse(args.iter().map(OsString::from))
    }
    #[test]
    fn accepts_original_directory_and_preflight_in_either_order() {
        let expected = DesktopArgs {
            original_dir: Some(PathBuf::from("D:\\老流星\\Meteor Game")),
            check_resources: true,
            ..Default::default()
        };
        assert_eq!(
            parse(&[
                "--original-dir",
                "D:\\老流星\\Meteor Game",
                "--check-resources"
            ])
            .unwrap(),
            expected
        );
        assert_eq!(
            parse(&[
                "--check-resources",
                "--original-dir=D:\\老流星\\Meteor Game"
            ])
            .unwrap(),
            expected
        );
        assert_eq!(parse(&[]).unwrap(), DesktopArgs::default());
    }
    #[test]
    fn bot_defaults_and_overrides_describe_a_dagger_room() {
        let options = parse(&[
            "--bot-host",
            "--bot-python",
            "py.exe",
            "--bot-script=tools/mrc_duel_host.py",
            "--bot-policy",
            "policy.pt",
            "--bot-stage",
            "sn23",
            "--bot-room=room",
            "--bot-name",
            "bot",
            "--bot-character",
            "2",
            "--bot-port",
            "6200",
        ])
        .unwrap();
        assert!(options.bot_host);
        assert_eq!(options.bot_python, Some(PathBuf::from("py.exe")));
        assert_eq!(
            options.bot_script,
            Some(PathBuf::from("tools/mrc_duel_host.py"))
        );
        assert_eq!(options.bot_policy, Some(PathBuf::from("policy.pt")));
        assert_eq!(options.bot_stage, "sn23");
        assert_eq!(options.bot_room, "room");
        assert_eq!(options.bot_name, "bot");
        assert_eq!(options.bot_character, 2);
        assert_eq!(options.bot_weapon, 5);
        assert_eq!(options.bot_port, 6200);
        let defaults = DesktopArgs::default();
        assert_eq!(defaults.bot_stage, "sn06");
        assert_eq!(defaults.bot_character, 0);
        assert_eq!(defaults.bot_port, 5230);
    }
    #[test]
    fn rejects_missing_duplicate_and_unknown_arguments() {
        for args in [
            vec!["--original-dir"],
            vec!["--original-dir="],
            vec!["--original-dir", "--check-resources"],
            vec!["--original-dir=a", "--original-dir=b"],
            vec!["--orginal-dir=a"],
            vec!["--bot-python"],
            vec!["--bot-port", "udp"],
            vec!["--bot-host=1"],
            vec!["--bot-weapon="],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
    #[test]
    fn resource_selection_obeys_build_default_and_explicit_override() {
        assert_eq!(
            parse(&[]).unwrap().resource_directory(),
            cfg!(feature = "original-directory-default").then(|| PathBuf::from("."))
        );
        assert_eq!(
            parse(&["--original-dir", "custom game"])
                .unwrap()
                .resource_directory(),
            Some(PathBuf::from("custom game"))
        );
        assert_eq!(
            parse(&["--check-resources"]).unwrap().resource_directory(),
            parse(&[]).unwrap().resource_directory()
        );
    }
    #[cfg(unix)]
    #[test]
    fn accepts_non_utf8_directory() {
        use std::os::unix::ffi::OsStringExt;
        let directory = OsString::from_vec(vec![b'g', b'a', b'm', b'e', 0xff]);
        let parsed =
            DesktopArgs::parse([OsString::from("--original-dir"), directory.clone()].into_iter())
                .unwrap();
        assert_eq!(parsed.original_dir, Some(PathBuf::from(directory)));
    }
}
