//! `shell-init`: the shell side of `fcd` and completion.
//!
//! Kept to a thin stub that asks the binary for everything, so a new build takes
//! effect in open shells without loading this again.

use crate::Output;

const ZSH: &str = r#"# forest: eval "$(forest shell-init zsh)" in .zshrc

fcd() {
  local dir
  dir=$(command forest path "$@") || return
  builtin cd -- "$dir"
}

_forest() {
  local -a cands
  cands=(${(f)"$(command forest __complete -- "${(@)words[1,CURRENT]}" 2>/dev/null)"})
  if [[ ${cands[1]} == :files ]]; then
    _files
  else
    compadd -a cands
  fi
}

# At the first prompt, after every compinit (a later one forgets earlier compdefs).
_forest_compdef() {
  add-zsh-hook -d precmd _forest_compdef
  (( $+functions[compdef] )) && compdef _forest forest fcd
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd _forest_compdef
"#;

pub fn shell_init() -> Output {
    Output::new(ZSH, serde_json::Value::Null)
}
