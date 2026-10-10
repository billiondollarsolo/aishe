//! Isolated ZDOTDIR contents and the zsh argv for the lean PTY child.

const HOOK_TEMPLATE: &str = include_str!("assets/hook.zsh");
const ZSHENV: &str = include_str!("assets/zshenv");
const PERSONAL_ZSHENV: &str = r#"# Native AIShe with the user's zsh configuration.
# Keep the wrapper ZDOTDIR until zsh finds our .zshrc, while allowing .zshenv
# to use and update the real directory exactly as it does outside AIShe.
ZDOTDIR="$AISHE_REAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"
export AISHE_REAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
ZDOTDIR="$AISHE_OUR_ZDOTDIR"
setopt RCS
"#;
const PERSONAL_ZPROFILE: &str = r#"# Forward the login environment before the interactive hook.
ZDOTDIR="$AISHE_REAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zprofile" ]] && source "$ZDOTDIR/.zprofile"
export AISHE_REAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
ZDOTDIR="$AISHE_OUR_ZDOTDIR"
"#;
const PERSONAL_ZLOGIN: &str = r#"# Restore the real directory for the remainder of this login shell.
ZDOTDIR="$AISHE_REAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zlogin" ]] && source "$ZDOTDIR/.zlogin"
"#;
const PERSONAL_ZLOGOUT: &str = r#"# Preserve the user's login-shell cleanup.
[[ -r "${ZDOTDIR:-$HOME}/.zlogout" ]] && source "${ZDOTDIR:-$HOME}/.zlogout"
"#;
const PERSONAL_ZSHRC: &str = r#"# User configuration runs before AIShe wraps its widgets.
ZDOTDIR="$AISHE_REAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshrc" ]] && source "$ZDOTDIR/.zshrc"
export AISHE_REAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
[[ -o login ]] && ZDOTDIR="$AISHE_OUR_ZDOTDIR"
"#;
const QUESTION_GRAMMAR_MARKER: &str = "# __AISHE_GENERATED_QUESTION_GRAMMAR__";
const SLASH_CATALOGUE_MARKER: &str = "# __AISHE_GENERATED_SLASH_CATALOGUE__";

pub fn wrapper_zshenv() -> &'static str {
    ZSHENV
}

/// A shell profile controls zsh customization, independently of the AI engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZshProfile {
    #[default]
    Clean,
    Personal,
}

impl ZshProfile {
    pub fn parse(value: Option<&str>) -> anyhow::Result<Self> {
        match value.unwrap_or("clean") {
            "clean" => Ok(Self::Clean),
            "personal" => Ok(Self::Personal),
            value => anyhow::bail!("invalid AISHE_ZSH_PROFILE {value:?}; choose clean or personal"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Personal => "personal",
        }
    }
}

pub fn wrapper_zshenv_for_profile(profile: ZshProfile) -> &'static str {
    match profile {
        ZshProfile::Clean => wrapper_zshenv(),
        ZshProfile::Personal => PERSONAL_ZSHENV,
    }
}

pub fn wrapper_zprofile_for_profile(profile: ZshProfile) -> &'static str {
    match profile {
        ZshProfile::Clean => "# Clean AIShe does not load a personal login profile.\n",
        ZshProfile::Personal => PERSONAL_ZPROFILE,
    }
}

pub fn wrapper_zlogin_for_profile(profile: ZshProfile) -> &'static str {
    match profile {
        ZshProfile::Clean => "# Clean AIShe keeps login startup isolated.\n",
        ZshProfile::Personal => PERSONAL_ZLOGIN,
    }
}

pub fn wrapper_zlogout_for_profile(profile: ZshProfile) -> &'static str {
    match profile {
        ZshProfile::Clean => "# Clean AIShe has no personal login cleanup.\n",
        ZshProfile::Personal => PERSONAL_ZLOGOUT,
    }
}

pub fn wrapper_zshrc_for_profile(profile: ZshProfile) -> String {
    match profile {
        ZshProfile::Clean => wrapper_zshrc(),
        ZshProfile::Personal => format!("{PERSONAL_ZSHRC}\n{}", wrapper_zshrc()),
    }
}

pub fn wrapper_zshrc() -> String {
    let grammar = crate::integration::question_grammar();
    assert_eq!(
        HOOK_TEMPLATE.matches(QUESTION_GRAMMAR_MARKER).count(),
        1,
        "lean hook must contain exactly one question-grammar marker"
    );
    let rendered = HOOK_TEMPLATE.replacen(QUESTION_GRAMMAR_MARKER, &grammar, 1);
    assert_eq!(HOOK_TEMPLATE.matches(SLASH_CATALOGUE_MARKER).count(), 1);
    let rendered = rendered.replacen(SLASH_CATALOGUE_MARKER, &super::slash::hook_catalogue(), 1);
    assert!(
        !rendered.contains("__AISHE_GENERATED_"),
        "lean hook has an unresolved generated marker"
    );
    rendered
}

/// `zsh -f` plus the options needed to source *this* isolated ZDOTDIR and skip
/// global rcs. `-f` is NO_RCS; `-o RCS` re-enables only ZDOTDIR files so the
/// lean hook loads; `-o NO_GLOBAL_RCS` keeps `/etc/zshrc` off.
pub fn zsh_argv() -> &'static [&'static str] {
    &["-f", "-o", "RCS", "-o", "NO_GLOBAL_RCS", "-i"]
}

pub fn zsh_argv_for_profile(profile: ZshProfile) -> &'static [&'static str] {
    match profile {
        ZshProfile::Clean => zsh_argv(),
        ZshProfile::Personal => &["-i"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_selection_is_explicit_and_independent() {
        assert_eq!(ZshProfile::parse(None).unwrap(), ZshProfile::Clean);
        assert_eq!(ZshProfile::parse(Some("clean")).unwrap(), ZshProfile::Clean);
        assert_eq!(
            ZshProfile::parse(Some("personal")).unwrap(),
            ZshProfile::Personal
        );
        for invalid in ["", "user", "legacy", "personal; echo unsafe"] {
            assert!(ZshProfile::parse(Some(invalid)).is_err());
        }
    }

    #[test]
    fn only_personal_wrappers_source_user_configuration() {
        assert!(!wrapper_zshenv_for_profile(ZshProfile::Clean).contains("AISHE_REAL_ZDOTDIR"));
        assert!(!wrapper_zshrc_for_profile(ZshProfile::Clean).contains("AISHE_REAL_ZDOTDIR"));
        assert!(wrapper_zshenv_for_profile(ZshProfile::Personal)
            .contains("source \"$ZDOTDIR/.zshenv\""));
        let rc = wrapper_zshrc_for_profile(ZshProfile::Personal);
        assert!(
            rc.find("source \"$ZDOTDIR/.zshrc\"").unwrap()
                < rc.find("aishe-accept-line()").unwrap()
        );
        assert_eq!(zsh_argv_for_profile(ZshProfile::Personal), ["-i"]);
    }
}
