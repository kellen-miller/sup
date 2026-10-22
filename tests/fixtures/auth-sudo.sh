#!/bin/sh
# Synthetic sudo only. PATH deliberately contains no system sudo.
# ps reports the actual controlling PTY; tty </dev/tty can report the alias /dev/tty.
scope=$(/bin/ps -o tty= -p $$ | /usr/bin/tr -d '[:space:]')
[ -n "$scope" ] || exit 74
scope=${scope##*/}
/bin/ps -o pgid= -p $$ >> "$FIXTURE_ROOT/job-groups"
if [ "${FIXTURE_SCOPE:-tty}" = parent ]; then scope=p$PPID; fi
coords=0
case "${SUDO_ASKPASS:-}" in
    */askpass-*)
        coords=1
        printf '%s\n%s\n' "${SUDO_ASKPASS%/*}/s" "${SUDO_ASKPASS##*/askpass-}" > "$FIXTURE_ROOT/coordinates"
        ;;
esac
printf 'sudo %s scope=%s coords=%s\n' "$*" "$scope" "$coords" >> "$FIXTURE_ROOT/trace"
/usr/bin/env >> "$FIXTURE_ROOT/observed-environment"
if [ "${FIXTURE_CANCEL_RETRY:-}" = yes ]; then
    trap '' TERM
    "$SUDO_ASKPASS" First: >/dev/null
    "$SUDO_ASKPASS" Later: >/dev/null
    printf 'later-denied=%s\n' "$?" >> "$FIXTURE_ROOT/trace"
    /bin/sleep 30 &
    wait
    exit 1
fi
if [ "$1" = -n ]; then
    if [ "${FIXTURE_PROBE:-}" = hang ]; then
        echo $$ > "$FIXTURE_ROOT/probe-pid"
        trap '' TERM
        /bin/sleep 30 &
        wait
        exit 1
    fi
    if [ "${FIXTURE_PROBE:-}" = fail ]; then exit 1; fi
    if [ "${FIXTURE_REFRESH:-}" = fail ] && [ -f "$FIXTURE_ROOT/probed" ]; then exit 1; fi
    : > "$FIXTURE_ROOT/probed"
    if [ -f "$FIXTURE_ROOT/ticket-$scope" ]; then
        echo probe-ticket=present >> "$FIXTURE_ROOT/trace"
        exit 0
    fi
    echo probe-ticket=absent >> "$FIXTURE_ROOT/trace"
    exit 1
fi
if [ "$1" != -A ]; then exit 71; fi
if [ -f "$FIXTURE_ROOT/ticket-$scope" ] && [ "${FIXTURE_ALWAYS_PROMPT:-}" != yes ]; then exit 0; fi
for attempt in 1 2 3; do
    printf 'askpass\n' >> "$FIXTURE_ROOT/trace"
    secret=$("$SUDO_ASKPASS" "$(printf '\033[31mPassword:\033[0m\033]0;untrusted\007')") || exit 72
    # The fixture checks only length. No password literal, argv, log, or environment.
    if [ "${#secret}" = 23 ]; then
        unset secret
        : > "$FIXTURE_ROOT/ticket-$scope"
        exit 0
    fi
    unset secret
    echo retry >> "$FIXTURE_ROOT/trace"
done
exit 73
