#!/bin/sh
printf 'brew-start\nreset\n' >> "$FIXTURE_ROOT/trace"
/bin/rm -f "$FIXTURE_ROOT"/ticket-*
sudo -A -v || exit $?
/bin/sleep 0.55
if [ "${FIXTURE_EXPIRE:-}" = yes ]; then /bin/rm -f "$FIXTURE_ROOT"/ticket-*; fi
sudo -A -v || exit $?
/bin/sleep "${FIXTURE_DURATION:-0.1}"
echo fixture-brew-complete
