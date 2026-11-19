#!/bin/sh
# Fed to `sh -s` on the remote machine over ssh: reports what gliff needs there.
# Output is key=value lines. Keep it POSIX sh: the remote login shell may be anything, and this
# runs before we know what the machine is. Every command reads /dev/null so none of them can
# swallow the rest of this script from stdin.
#
# The caller may prepend `server_bin='...'` when the user set a custom gliff-server command; its
# presence is then trusted, not checked.

: "${XDG_RUNTIME_DIR:=/run/user/$(id -u 2>/dev/null)}"
export XDG_RUNTIME_DIR

echo "os=$(uname -s 2>/dev/null)"

hypr=0
outputs=
if command -v hyprctl >/dev/null 2>&1; then
  # Over ssh there is no HYPRLAND_INSTANCE_SIGNATURE; `-i 0` picks the first running instance,
  # the same session gliff-server finds.
  if [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]; then set --; else set -- -i 0; fi
  if hyprctl "$@" version >/dev/null 2>&1 </dev/null; then
    hypr=1
    outputs=$(hyprctl "$@" monitors 2>/dev/null </dev/null |
      sed -n 's/^Monitor \([^ ]*\) .*/\1/p' | tr '\n' ' ')
  fi
fi
if [ "$hypr" = 0 ] && pgrep -x Hyprland >/dev/null 2>&1 </dev/null; then hypr=1; fi
echo "hyprland=$hypr"
echo "outputs=$outputs"

if [ -n "${server_bin:-}" ]; then
  echo "gliff_server=custom"
else
  server_path=$(command -v gliff-server 2>/dev/null </dev/null)
  echo "gliff_server=$server_path"
  if [ -n "$server_path" ]; then
    echo "server_version=$(gliff-server --version 2>/dev/null </dev/null | head -n 1)"
  fi
fi
exit 0
