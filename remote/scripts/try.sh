#!/bin/sh
command=$1
shift
case $command in
  setup)
    set -e
    mkdir -p "$1"
    cd "$1"
    git init -q
    printf 'base\n' > notes.txt
    git add notes.txt
    git -c user.name="magi try" -c user.email=try@magi.invalid commit -q -m base
    printf 'uncommitted line\n' >> notes.txt
    ;;
  cleanup)
    project=$1 id=$2
    shift 2
    for root; do
      case $root in \~/*) root=$HOME/${root#\~/} ;; esac
      rm -rf -- "${root:?}/$project"
    done
    rm -rf -- "$HOME/.claude/projects/"*"$project"
    [ -z "$id" ] || rm -f -- "$HOME/.magi/moved/$id"
    ;;
esac
