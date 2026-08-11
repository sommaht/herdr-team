//! Where the two config layers are looked for.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// Where the user's file sits under a config directory.
const RELATIVE_PATH: &str = "herdr-team/config.toml";

/// Where a repository carries its own, relative to any directory in it, in the order they are tried.
///
/// The order settles only a tie inside one directory; which form is *nearer* decides across them.
const REPOSITORY_PATHS: [&str; 2] = [".herdr-team/config.toml", ".herdr-team.config.toml"];

// =====================================================================================================================
// The User Layer
// =====================================================================================================================

/// The user layer's path, and whether the caller named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserPath {
    /// `--config` or the environment override.
    Named(PathBuf),
    /// The config directory's default location.
    Default(PathBuf),
}

impl UserPath {
    /// The path itself.
    pub fn path(&self) -> &Path {
        match self {
            Self::Named(path) | Self::Default(path) => path,
        }
    }

    /// Whether the caller named it, which decides whether its absence is a failure.
    pub fn named(&self) -> bool {
        matches!(self, Self::Named(_))
    }
}

/// Where the user's config file is looked for, in order: `--config`, the environment override,
/// `$XDG_CONFIG_HOME`, then `~/.config`.
pub fn user(
    explicit: Option<&Path>,
    environment: Option<&OsStr>,
    xdg: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<UserPath> {
    if let Some(path) = explicit {
        return Some(UserPath::Named(path.to_owned()));
    }
    if let Some(path) = environment {
        return Some(UserPath::Named(PathBuf::from(path)));
    }
    if let Some(directory) = xdg {
        return Some(UserPath::Default(Path::new(directory).join(RELATIVE_PATH)));
    }
    home.map(|directory| UserPath::Default(Path::new(directory).join(".config").join(RELATIVE_PATH)))
}

// =====================================================================================================================
// The Repository Layer
// =====================================================================================================================

/// The nearest repository config at or above `cwd`, walking to the filesystem root.
///
/// The walk does not stop at a repository boundary. Both spellings in [`REPOSITORY_PATHS`] are tried
/// at each ancestor before the walk moves up, so a nearer config wins whichever form it takes.
pub fn repository(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .flat_map(|directory| REPOSITORY_PATHS.map(|relative| directory.join(relative)))
        .find(|candidate| candidate.is_file())
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Writes a config at `path`, making whatever directories it needs first.
    fn write_config(path: &Path) {
        fs::create_dir_all(path.parent().expect("a config path has a parent")).unwrap();
        fs::write(path, "default = 'x'\n").unwrap();
    }

    #[test]
    fn the_search_order_is_explicit_then_the_environment_then_xdg_then_home() {
        let explicit = PathBuf::from("/explicit/config.toml");

        assert_eq!(
            user(
                Some(&explicit),
                Some("/env".as_ref()),
                Some("/xdg".as_ref()),
                Some("/home".as_ref())
            ),
            Some(UserPath::Named(explicit))
        );
        assert_eq!(
            user(
                None,
                Some("/env/p.toml".as_ref()),
                Some("/xdg".as_ref()),
                Some("/home".as_ref())
            ),
            Some(UserPath::Named(PathBuf::from("/env/p.toml")))
        );
        assert_eq!(
            user(None, None, Some("/xdg".as_ref()), Some("/home".as_ref())),
            Some(UserPath::Default(PathBuf::from("/xdg/herdr-team/config.toml")))
        );
        assert_eq!(
            user(None, None, None, Some("/home".as_ref())),
            Some(UserPath::Default(PathBuf::from("/home/.config/herdr-team/config.toml")))
        );
        assert_eq!(user(None, None, None, None), None);
    }

    #[test]
    fn only_a_path_the_caller_named_is_one_this_tool_must_find() {
        assert!(user(Some(Path::new("/a.toml")), None, None, None).unwrap().named());
        assert!(user(None, Some("/b.toml".as_ref()), None, None).unwrap().named());
        assert!(!user(None, None, Some("/xdg".as_ref()), None).unwrap().named());
        assert!(!user(None, None, None, Some("/home".as_ref())).unwrap().named());
    }

    #[test]
    fn a_repository_config_is_found_from_any_directory_beneath_it() {
        for relative in REPOSITORY_PATHS {
            let root = tempfile::tempdir().unwrap();
            let config = root.path().join(relative);
            write_config(&config);
            let deep = root.path().join("src/api/handlers");
            fs::create_dir_all(&deep).unwrap();

            assert_eq!(repository(&deep), Some(config.clone()), "from beneath {relative}");
            assert_eq!(repository(root.path()), Some(config), "from beside {relative}");
        }
    }

    #[test]
    fn the_nearest_one_wins_over_an_ancestors() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join(".herdr-team/config.toml");
        let inner = root.path().join("nested/.herdr-team/config.toml");
        for path in [&outer, &inner] {
            write_config(path);
        }

        assert_eq!(repository(&root.path().join("nested")), Some(inner));
        assert_eq!(repository(root.path()), Some(outer));
    }

    #[test]
    fn the_directory_form_wins_over_a_flat_file_in_the_same_directory() {
        let root = tempfile::tempdir().unwrap();
        let directory_form = root.path().join(".herdr-team/config.toml");
        write_config(&directory_form);
        write_config(&root.path().join(".herdr-team.config.toml"));

        assert_eq!(repository(root.path()), Some(directory_form));
    }

    #[test]
    fn a_nearer_flat_file_beats_a_directory_form_further_up() {
        let root = tempfile::tempdir().unwrap();
        write_config(&root.path().join(".herdr-team/config.toml"));
        let nested = root.path().join("nested");
        let flat_form = nested.join(".herdr-team.config.toml");
        write_config(&flat_form);

        assert_eq!(repository(&nested), Some(flat_form));
    }

    #[test]
    fn a_tree_with_no_repository_config_has_none_rather_than_a_path_that_is_not_there() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(repository(root.path()), None);
    }

    #[test]
    fn a_directory_where_the_file_should_be_is_not_a_config() {
        for relative in REPOSITORY_PATHS {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir_all(root.path().join(relative)).unwrap();

            assert_eq!(repository(root.path()), None, "a directory named {relative}");
        }
    }
}
