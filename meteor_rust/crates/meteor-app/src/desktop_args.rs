use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DesktopArgs {
    pub original_dir: Option<PathBuf>,
    pub check_resources: bool,
    pub bake_map_previews: bool,
    pub help: bool,
}

impl DesktopArgs {
    pub fn resource_directory(&self) -> Option<PathBuf> {
        self.original_dir
            .clone()
            .or_else(|| cfg!(feature = "original-directory-default").then(|| PathBuf::from(".")))
    }

    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let path = if arg == "--original-dir" {
                Some(
                    args.next()
                        .filter(|value| {
                            !value.is_empty() && !value.to_string_lossy().starts_with("--")
                        })
                        .ok_or("--original-dir requires a directory path")?,
                )
            } else if let Some(value) = arg.to_str().and_then(|s| s.strip_prefix("--original-dir="))
            {
                if value.is_empty() {
                    return Err("--original-dir requires a directory path".into());
                }
                Some(value.into())
            } else if arg == "--bake-map-previews" {
                options.bake_map_previews = true;
                None
            } else if arg == "--check-resources" {
                options.check_resources = true;
                None
            } else if arg == "--help" || arg == "-h" {
                options.help = true;
                None
            } else {
                return Err(format!("Unknown argument: {}", arg.to_string_lossy()));
            };
            if let Some(path) = path {
                if options.original_dir.replace(PathBuf::from(path)).is_some() {
                    return Err("--original-dir may only be specified once".into());
                }
            }
        }
        Ok(options)
    }
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
            bake_map_previews: false,
            help: false,
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
    fn rejects_missing_duplicate_and_unknown_arguments() {
        for args in [
            vec!["--original-dir"],
            vec!["--original-dir="],
            vec!["--original-dir", "--check-resources"],
            vec!["--original-dir=a", "--original-dir=b"],
            vec!["--orginal-dir=a"],
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
        let path = OsString::from_vec(vec![b'/', 255]);
        assert_eq!(
            DesktopArgs::parse(["--original-dir".into(), path.clone()])
                .unwrap()
                .original_dir,
            Some(path.into())
        );
    }
}
