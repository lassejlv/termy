// The zsh side of Termy Next's shell plugins. ShellPlugins writes these to
// <support>/shell/zsh and points ZDOTDIR there for new panes.

enum ShellScripts {
    /// Puts the user's ZDOTDIR back, runs their .zshenv, then loads the plugin.
    /// zsh reads the rest of the startup files from the restored ZDOTDIR.
    static let zshenv = #"""
    # Termy Next set ZDOTDIR to this folder to load its zsh plugins. Put the
    # real ZDOTDIR back, run your own .zshenv, then load the plugins.
    typeset -g _termy_next_dir=${${(%):-%x}:A:h}
    if [[ -n ${TERMY_NEXT_ZDOTDIR+x} ]]; then
      export ZDOTDIR=$TERMY_NEXT_ZDOTDIR
    else
      unset ZDOTDIR
    fi
    unset TERMY_NEXT_ZDOTDIR
    [[ -r ${ZDOTDIR:-$HOME}/.zshenv ]] && builtin source ${ZDOTDIR:-$HOME}/.zshenv
    [[ -o interactive && -r $_termy_next_dir/termy-next.zsh ]] && builtin source $_termy_next_dir/termy-next.zsh
    unset _termy_next_dir
    """#

    /// Reports the command line to Termy Next as you type, with matching
    /// history and commands, and takes back lines to set or run.
    static let plugin = #"""
    # Termy Next's zsh integration.
    [[ -o interactive ]] || return 0
    (( ${+_termy_next_loaded} )) && return 0

    # ssh sends COLORTERM along, so programs on the other end (Claude Code,
    # vim, bat) draw 24-bit color instead of guessing from TERM. The server
    # takes it when its sshd_config has AcceptEnv COLORTERM, as Debian and
    # Ubuntu ship. SendEnv adds to your ssh config; it doesn't replace it.
    if [[ -n $TERMY_NEXT_SSH_ENV && -n $COLORTERM ]] && (( ! ${+functions[ssh]} )); then
      ssh() { command ssh -o SendEnv=COLORTERM "$@" }
    fi

    # The rest is the GUI plugins: the completion popup under the prompt and
    # the ⇧⌘E command box. Talks to Termy Next over $TERMY_NEXT_SOCKET; does
    # nothing when Termy Next isn't listening.
    [[ -n $TERMY_NEXT_SOCKET ]] || return 0
    typeset -g _termy_next_loaded=1
    zmodload zsh/net/socket zsh/parameter zsh/datetime 2>/dev/null || return 0
    autoload -Uz add-zsh-hook add-zle-hook-widget

    typeset -g _termy_next_fd= _termy_next_last= _termy_next_tried=0
    typeset -g _termy_next_us=$'\x1f' _termy_next_rs=$'\x1e'
    typeset -ga _termy_next_names

    # One protocol field in REPLY: separators dropped, \ and newlines escaped.
    _termy_next_escape() {
      emulate -L zsh
      local s=${1//[$'\x1f\x1e']/}
      s=${s//\\/\\\\}
      REPLY=${s//$'\n'/\\n}
    }

    # SIGPIPE is ignored for this write only, so Termy Next quitting can't end
    # the shell, and programs started later still get the default.
    _termy_next_send() {
      emulate -L zsh
      [[ -n $_termy_next_fd ]] || return 1
      trap '' PIPE
      print -r -- "$1" >&$_termy_next_fd 2>/dev/null && return 0
      _termy_next_disconnect
      return 1
    }

    _termy_next_disconnect() {
      emulate -L zsh
      [[ -n $_termy_next_fd ]] || return 0
      zle -F $_termy_next_fd 2>/dev/null
      exec {_termy_next_fd}>&-
      _termy_next_fd=
      _termy_next_last=
    }

    _termy_next_connect() {
      emulate -L zsh
      [[ -n $_termy_next_fd ]] && return 0
      _termy_next_tried=$EPOCHSECONDS
      [[ -S $TERMY_NEXT_SOCKET ]] || return 1
      zsocket $TERMY_NEXT_SOCKET 2>/dev/null || return 1
      _termy_next_fd=$REPLY
      zle -F -w $_termy_next_fd _termy_next_receive 2>/dev/null
      local pane search_path
      _termy_next_escape "${TERMY_NEXT_PANE:-$TERMY_MUX_PANE}"; pane=$REPLY
      _termy_next_escape "$PATH"; search_path=$REPLY
      _termy_next_send "hello$_termy_next_us$pane$_termy_next_us$$$_termy_next_us$search_path"
    }

    # A message from Termy Next: a command line to set. Termy Next runs it by
    # typing Return once the line is reported back; accept-line from a handler
    # would wait for the next key.
    _termy_next_receive() {
      emulate -L zsh
      local fd=${1:-$_termy_next_fd} message
      if [[ -n $2 ]] || ! IFS= read -r -u $fd message; then
        _termy_next_disconnect
        return 0
      fi
      local verb=${message%%$_termy_next_us*}
      local text=${message#*$_termy_next_us}
      text=${(g::)text}
      case $verb in
        # Reported back at once: a handler's redraw skips line-pre-redraw.
        set) BUFFER=$text; CURSOR=$#BUFFER; zle -R; _termy_next_report ;;
      esac
    }

    # Sends the line and its suggestions whenever it changes.
    _termy_next_report() {
      emulate -L zsh
      if [[ -z $_termy_next_fd ]]; then
        (( EPOCHSECONDS - _termy_next_tried >= 2 )) || return 0
        _termy_next_connect || return 0
      fi
      [[ $CONTEXT == start ]] || return 0
      local state="$CURSOR:$BUFFER"
      [[ $state == "$_termy_next_last" ]] && return 0
      _termy_next_last=$state
      local -a hist cmds out
      local item history_field command_field
      if (( CURSOR == $#BUFFER )) && [[ $BUFFER == *[^[:space:]]* ]]; then
        hist=(${(u)${(M)${(v)history}:#${BUFFER}*}})
        hist=(${hist[1,5]})
        if [[ $BUFFER != *[[:space:]]* ]]; then
          cmds=(${(ou)${(M)_termy_next_names:#${BUFFER}*}})
          cmds=(${cmds[1,5]})
        fi
      fi
      for item in $hist; do _termy_next_escape "$item"; out+=($REPLY); done
      history_field=${(pj:$_termy_next_rs:)out}
      out=()
      for item in $cmds; do _termy_next_escape "$item"; out+=($REPLY); done
      command_field=${(pj:$_termy_next_rs:)out}
      _termy_next_escape "$BUFFER"
      _termy_next_send "line$_termy_next_us$CURSOR$_termy_next_us$REPLY$_termy_next_us$history_field$_termy_next_us$command_field"
    }

    _termy_next_finish() {
      emulate -L zsh
      _termy_next_last=
      [[ -n $_termy_next_fd ]] && _termy_next_send done
      return 0
    }

    _termy_next_precmd() {
      emulate -L zsh
      _termy_next_names=(${(k)commands} ${(k)aliases} ${(k)builtins} ${${(k)functions}:#[_.+-]*})
      _termy_next_connect
      return 0
    }

    # Hooks go in at the first prompt, after .zshrc has set up its own widgets.
    _termy_next_install() {
      add-zsh-hook -d precmd _termy_next_install
      zle -N _termy_next_receive
      zle -N _termy_next_report
      zle -N _termy_next_finish
      add-zle-hook-widget line-pre-redraw _termy_next_report
      add-zle-hook-widget line-finish _termy_next_finish
      add-zsh-hook precmd _termy_next_precmd
      _termy_next_precmd
    }
    add-zsh-hook precmd _termy_next_install
    """#
}
