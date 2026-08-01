# aishe zsh integration — add to ~/.zshrc:  eval "$(aishe init zsh)"
# Routes unknown input to aishe. Native ZLE (autosuggestions, syntax
# highlighting, oh-my-zsh) is untouched and works as usual.
# Set AISHE_MODE=suggest|auto|yolo to control behavior (default: suggest).
# In auto mode, safe commands run directly (cd/export persist); dangerous ones
# are pre-filled for review. Press Alt-Enter (or $AISHE_NL_KEY) to force a line
# to be treated as natural language, and Shift-Tab (or $AISHE_MODE_KEY) to cycle
# the mode for the session.
# __AISHE_TEMPLATE_ZSH_HOOK_FINAL__
