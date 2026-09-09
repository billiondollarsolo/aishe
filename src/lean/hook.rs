//! Isolated ZDOTDIR contents and the zsh argv for the lean PTY child.

const HOOK_TEMPLATE: &str = include_str!("assets/hook.zsh");
const ZSHENV: &str = include_str!("assets/zshenv");
const QUESTION_GRAMMAR_MARKER: &str = "# __AISHE_GENERATED_QUESTION_GRAMMAR__";

pub fn wrapper_zshenv() -> &'static str {
    ZSHENV
}

pub fn wrapper_zshrc() -> String {
    let grammar = crate::integration::question_grammar();
    assert_eq!(
        HOOK_TEMPLATE.matches(QUESTION_GRAMMAR_MARKER).count(),
        1,
        "lean hook must contain exactly one question-grammar marker"
    );
    let rendered = HOOK_TEMPLATE.replacen(QUESTION_GRAMMAR_MARKER, &grammar, 1);
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
