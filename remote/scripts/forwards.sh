#!/bin/sh
[ -f "$HOME/.magi/forwards/$1.json" ] || exit 0
exec "$HOME/.magi/bin/magi" forwards close "$1"
