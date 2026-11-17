#!/bin/sh
for host; do
  printf '%s\0' "$(ssh -G -- "$host" 2>/dev/null | awk '$1 == "hostname" {print $2; exit}')"
done
